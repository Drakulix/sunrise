use std::{
    os::unix::io::{FromRawFd, OwnedFd},
    sync::Mutex,
};

use gst::prelude::Cast;
use gst::subclass::prelude::*;
use gst::{glib, traits::AllocatorExt};

use gst_video::{VideoBufferPoolConfig, VideoInfo};
use once_cell::sync::Lazy;
use smithay::backend::allocator::{
    dmabuf::{Dmabuf, DmabufFlags},
    Modifier,
};

use crate::allocators::GbmMemoryAllocator;
use crate::utils::gst_video_format_to_drm_fourcc;

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "smithaybufferpool",
        gst::DebugColorFlags::empty(),
        Some("Wayland Buffer Pool"),
    )
});

#[derive(Debug, Default)]
pub struct State {
    video_info: Option<VideoInfo>,
    allocator: Option<gst::Allocator>,
    allocation_params: Option<Option<gst::AllocationParams>>,
    add_video_meta: bool,
}

#[derive(Debug, Default)]
pub struct SmithayBufferPool {
    pub state: Mutex<State>,
}

#[glib::object_subclass]
impl ObjectSubclass for SmithayBufferPool {
    const NAME: &'static str = "SmithayBufferPool";
    type Type = super::SmithayBufferPool;
    type ParentType = gst::BufferPool;
    type Interfaces = ();
}

impl ObjectImpl for SmithayBufferPool {}

impl GstObjectImpl for SmithayBufferPool {}

impl BufferPoolImpl for SmithayBufferPool {
    fn options() -> &'static [&'static str] {
        static OPTIONS: Lazy<Vec<&'static str>> = Lazy::new(|| {
            vec![
                &*gst_video::BUFFER_POOL_OPTION_VIDEO_META,
                &*gst_video::BUFFER_POOL_OPTION_VIDEO_ALIGNMENT,
            ]
        });

        OPTIONS.as_ref()
    }

    fn alloc_buffer(
        &self,
        params: Option<&gst::BufferPoolAcquireParams>,
    ) -> Result<gst::Buffer, gst::FlowError> {
        let state = self.state.lock().unwrap();
        let video_info = state.video_info.as_ref().unwrap();
        let allocator = state.allocator.as_ref().unwrap();

        let mut buffer = if let Some(gbm_allocator) = allocator.downcast_ref::<GbmMemoryAllocator>()
        {
            let mem = match gbm_allocator.alloc(video_info.size(), None) {
                Ok(mem) => mem,
                Err(_) => {
                    return Err(gst::FlowError::Error);
                }
            };

            let mut buffer = gst::Buffer::new();
            let buffer_mut = buffer.make_mut();
            buffer_mut.insert_memory(None, mem);
            buffer
        } else {
            self.parent_alloc_buffer(params)?
        };

        let mem = buffer.memory(0).unwrap();
        if mem
            .downcast_memory_ref::<gst_allocators::DmaBufMemory>()
            .is_some()
        {
            let Some(format) = gst_video_format_to_drm_fourcc(video_info.format()) else {
                return Err(gst::FlowError::Error);
            };

            let mut dmabuf = Dmabuf::builder(
                (video_info.width() as i32, video_info.height() as i32),
                format,
                DmabufFlags::empty(),
            );

            for plane in 0..video_info.n_planes() {
                let offset = video_info.offset()[plane as usize];
                let stride = video_info.stride()[plane as usize];

                let (mem_idx, _, skip) = buffer
                    .find_memory(offset, Some(1))
                    .expect("memory does not seem to contain enough data for the specified format");
                let mem = buffer
                    .peek_memory(mem_idx)
                    .downcast_memory_ref::<gst_allocators::DmaBufMemory>()
                    .unwrap();

                if !dmabuf.add_plane(
                    unsafe { OwnedFd::from_raw_fd(mem.fd()) },
                    plane,
                    (mem.offset() + skip) as u32,
                    stride as u32,
                    Modifier::Linear,
                ) {
                    gst::warning!(CAT, imp: self, "failed to add plane");
                    return Err(gst::FlowError::Error);
                }
            }

            let Some(dmabuf) = dmabuf.build() else {
                gst::warning!(CAT, imp: self, "failed to finish dmabuf");
                return Err(gst::FlowError::Error)
            };

            let buffer_mut = buffer.make_mut();
            super::meta::SmithayBufferMeta::add(buffer_mut, dmabuf);
            if state.add_video_meta {
                gst_video::VideoMeta::add_full(
                    buffer_mut,
                    gst_video::VideoFrameFlags::empty(),
                    video_info.format(),
                    video_info.width(),
                    video_info.height(),
                    video_info.offset(),
                    video_info.stride(),
                )
                .map_err(|err| {
                    gst::warning!(CAT, imp: self, "failed to add video meta: {:?}", err);
                    gst::FlowError::Error
                })?;
            }
            buffer_mut.unset_flags(gst::BufferFlags::TAG_MEMORY);

            return Ok(buffer);
        }

        Err(gst::FlowError::Error)
    }

    fn set_config(&self, config: &mut gst::BufferPoolConfigRef) -> bool {
        let (caps, size, min_buffers, max_buffers) = match config.params() {
            Some(params) => params,
            None => {
                gst::warning!(CAT, imp: self, "no params");
                return false;
            }
        };

        let caps = match caps {
            Some(caps) => caps,
            None => {
                gst::warning!(CAT, imp: self, "no caps config");
                return false;
            }
        };

        let mut video_info = match VideoInfo::from_caps(&caps) {
            Ok(info) => info,
            Err(err) => {
                gst::warning!(
                    CAT,
                    imp: self,
                    "failed to get video info from caps: {}",
                    err
                );
                return false;
            }
        };

        let (allocator, mut allocation_params) =
            if let Some((allocator, allocation_params)) = config.allocator() {
                let Some(allocator) = allocator else {
                    gst::warning!(
                        CAT,
                        imp: self,
                        "failed to get allocator",
                    );
                    return false;
                };
                (allocator, Some(allocation_params))
            } else {
                gst::warning!(CAT, imp: self, "failed to get allocator",);
                return false;
            };

        let mut guard = self.state.lock().unwrap();
        guard.add_video_meta = config.has_option(gst_video::BUFFER_POOL_OPTION_VIDEO_META.as_ref());
        let need_alignment =
            config.has_option(gst_video::BUFFER_POOL_OPTION_VIDEO_ALIGNMENT.as_ref());

        if need_alignment && guard.add_video_meta {
            let video_align = config.video_alignment();

            if let Some(video_align) = video_align {
                let align = allocation_params
                    .as_ref()
                    .map(|params| params.align())
                    .unwrap_or_default();
                let mut max_align = align;

                for plane in 0..video_info.n_planes() {
                    max_align |= unsafe {
                        *video_align.stride_align().get_unchecked(plane as usize) as usize
                    };
                }

                let mut stride_align: [u32; gst_video::ffi::GST_VIDEO_MAX_PLANES as usize] =
                    [0; gst_video::ffi::GST_VIDEO_MAX_PLANES as usize];
                for plane in 0..video_info.n_planes() {
                    stride_align[plane as usize] = max_align as u32;
                }

                let mut video_align = gst_video::VideoAlignment::new(
                    video_align.padding_top(),
                    video_align.padding_bottom(),
                    video_align.padding_left(),
                    video_align.padding_right(),
                    &stride_align,
                );
                if let Err(err) = video_info.align(&mut video_align) {
                    gst::warning!(CAT, imp: self, "failed to align video info: {}", err);
                    return false;
                }

                config.set_video_alignment(&video_align);

                if align < max_align {
                    gst::warning!(CAT, imp: self, "allocation params alignment {} is smaller than the max specified video stride alignment {}, fixing", align, max_align);
                    allocation_params = allocation_params.as_ref().map(|params| {
                        gst::AllocationParams::new(
                            params.flags(),
                            max_align,
                            params.prefix(),
                            params.padding(),
                        )
                    });
                    config.set_allocator(Some(&allocator), allocation_params.as_ref());
                }
            }
        }

        let size = std::cmp::max(size, video_info.size() as u32);
        guard.video_info = Some(video_info);

        config.set_params(Some(&caps), size, min_buffers, max_buffers);

        guard.allocator = Some(allocator);
        guard.allocation_params = Some(allocation_params);

        self.parent_set_config(config)
    }

    fn free_buffer(&self, buffer: gst::Buffer) {
        let _ = buffer;
    }
}
