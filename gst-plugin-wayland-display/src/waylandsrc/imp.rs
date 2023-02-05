use std::collections::HashSet;
use std::sync::{
    mpsc::{self, SyncSender},
    Mutex,
};
use std::thread::JoinHandle;

use gst_video::{VideoBufferPoolConfig, VideoCapsBuilder, VideoInfo};
use slog::Drain;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::drm::{DrmNode, NodeType};
use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::reexports::calloop::channel::Sender;

use gst::glib;
use gst::glib::once_cell::sync::Lazy;
use gst::prelude::*;
use gst::subclass::prelude::*;

use gst_base::subclass::base_src::CreateSuccess;
use gst_base::subclass::prelude::*;
use gst_base::traits::BaseSrcExt;

use crate::allocators::GbmMemoryAllocator;
use crate::buffer_pool::{SmithayBufferMeta, SmithayBufferPool};
use crate::utils::gst_video_format_from_drm_fourcc;

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "waylanddisplaysrc",
        gst::DebugColorFlags::empty(),
        Some("Wayland Display Source Bin"),
    )
});

pub struct SlogGstDrain;

impl slog::Drain for SlogGstDrain {
    type Ok = ();
    type Err = std::convert::Infallible;

    fn log(
        &self,
        record: &slog::Record,
        _values: &slog::OwnedKVList,
    ) -> std::result::Result<Self::Ok, Self::Err> {
        CAT.log(
            Option::<&super::WaylandDisplaySrc>::None,
            match record.level() {
                slog::Level::Critical | slog::Level::Error => gst::DebugLevel::Error,
                slog::Level::Warning => gst::DebugLevel::Warning,
                slog::Level::Info => gst::DebugLevel::Info,
                slog::Level::Debug => gst::DebugLevel::Debug,
                slog::Level::Trace => gst::DebugLevel::Trace,
            },
            glib::GString::from(record.file()).as_gstr(),
            record.module(),
            record.line(),
            *record.msg(),
        );
        Ok(())
    }
}

pub struct WaylandDisplaySrc {
    egl_display: Mutex<Option<EGLDisplay>>,
    state: Mutex<Option<State>>,
    settings: Mutex<Settings>,
}

impl Default for WaylandDisplaySrc {
    fn default() -> Self {
        WaylandDisplaySrc {
            egl_display: Mutex::new(None),
            state: Mutex::new(None),
            settings: Mutex::new(Settings::default()),
        }
    }
}

#[derive(Debug, Default)]
pub struct Settings {
    render_node: Option<DrmNode>,
    input_seat: Option<String>,
}

pub struct State {
    thread_handle: JoinHandle<()>,
    command_tx: Sender<Command>,
}

pub enum Command {
    VideoInfo(VideoInfo),
    Buffer(Dmabuf, SyncSender<()>),
    Quit,
}

#[glib::object_subclass]
impl ObjectSubclass for WaylandDisplaySrc {
    const NAME: &'static str = "GstWaylandDisplaySrc";
    type Type = super::WaylandDisplaySrc;
    type ParentType = gst_base::PushSrc;
    type Interfaces = ();
}

impl ObjectImpl for WaylandDisplaySrc {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecString::builder("render-node")
                    .nick("DRM Render Node")
                    .blurb("DRM Render Node to use (e.g. /dev/dri/renderD128")
                    .construct()
                    .build(),
                glib::ParamSpecString::builder("seat")
                    .nick("libinput seat")
                    .blurb("libinput seat to use (e.g. seat-0")
                    .construct()
                    .build(),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "render-node" => {
                let mut settings = self.settings.lock().unwrap();
                let node = value
                    .get::<Option<String>>()
                    .expect("type checked upstream")
                    .map(|path| DrmNode::from_path(path).expect("No valid render_node"));
                settings.render_node = node;
            }
            "seat" => {
                let mut settings = self.settings.lock().unwrap();
                let seat = value
                    .get::<Option<String>>()
                    .expect("type checked upstream");
                settings.input_seat = seat;
            }
            _ => unreachable!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "render-node" => {
                let settings = self.settings.lock().unwrap();
                settings
                    .render_node
                    .as_ref()
                    .and_then(|node| node.dev_path())
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| String::from("/dev/dri/renderD128"))
                    .to_value()
            }
            "seat" => {
                let settings = self.settings.lock().unwrap();
                settings.input_seat.to_value()
            }
            _ => unreachable!(),
        }
    }

    fn constructed(&self) {
        self.parent_constructed();

        let obj = self.obj();
        obj.set_element_flags(gst::ElementFlags::SOURCE);
        obj.set_live(true);
        obj.set_format(gst::Format::Time);
        obj.set_automatic_eos(false);
        obj.set_do_timestamp(true);
    }
}

impl GstObjectImpl for WaylandDisplaySrc {}

fn get_egl_device_for_node(drm_node: DrmNode) -> EGLDevice {
    let drm_node = drm_node
        .node_with_type(NodeType::Render)
        .and_then(Result::ok)
        .unwrap_or(drm_node);
    EGLDevice::enumerate()
        .expect("Failed to enumerate EGLDevices")
        .find(|d| d.try_get_render_node().unwrap_or_default() == Some(drm_node))
        .expect("Unable to find EGLDevice for drm-node")
}

impl ElementImpl for WaylandDisplaySrc {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "Wayland display source",
                "Source/Video",
                "GStreamer video src running a wayland compositor",
                "Victoria Brekenfeld <wayland@drakulix.de>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let dmabuf_caps = gst_video::VideoCapsBuilder::new()
                .features([gst_allocators::CAPS_FEATURE_MEMORY_DMABUF])
                .format_list(gst_video::VIDEO_FORMATS_ALL.iter().copied())
                .build();
            let src_pad_template = gst::PadTemplate::new(
                "src",
                gst::PadDirection::Src,
                gst::PadPresence::Always,
                &dmabuf_caps,
            )
            .unwrap();

            vec![src_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }

    fn change_state(
        &self,
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        if transition.next() == gst::State::Null {
            self.egl_display.lock().unwrap().take();
        }

        let res = self.parent_change_state(transition);
        match res {
            Ok(gst::StateChangeSuccess::Success) => {
                if transition.next() == gst::State::Paused {
                    // this is a live source
                    Ok(gst::StateChangeSuccess::NoPreroll)
                } else {
                    Ok(gst::StateChangeSuccess::Success)
                }
            }
            x => x,
        }
    }

    fn query(&self, query: &mut gst::QueryRef) -> bool {
        ElementImplExt::parent_query(self, query)
    }
}

impl BaseSrcImpl for WaylandDisplaySrc {
    fn query(&self, query: &mut gst::QueryRef) -> bool {
        BaseSrcImplExt::parent_query(self, query)
    }

    fn caps(&self, filter: Option<&gst::Caps>) -> Option<gst::Caps> {
        let max_refresh = gst::Fraction::new(i32::MAX, 1);

        let settings = self.settings.lock().unwrap();
        let render_node = settings.render_node.clone().unwrap_or_else(|| {
            DrmNode::from_path("/dev/dri/renderD128")
                .expect("Failed to open default DRM render node")
        });

        let mut egl_display_guard = self.egl_display.lock().unwrap();
        let egl_display = match egl_display_guard.as_mut() {
            Some(display) => display,
            None => {
                let log = ::slog::Logger::root(SlogGstDrain.fuse(), slog::o!());
                let egl_device = get_egl_device_for_node(render_node);
                let egl_display =
                    EGLDisplay::new(egl_device, log).expect("Failed to open EGLDisplay");
                *egl_display_guard = Some(egl_display);
                egl_display_guard.as_mut().unwrap()
            }
        };

        let fourccs = egl_display
            .dmabuf_render_formats()
            .into_iter()
            .map(|format| format.code)
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(|fourcc| gst_video_format_from_drm_fourcc(fourcc));

        let mut dmabuf_caps = VideoCapsBuilder::new()
            .format_list(fourccs)
            .framerate_range(..max_refresh)
            .build();

        if let Some(filter) = filter {
            dmabuf_caps = dmabuf_caps.intersect(filter);
        }

        Some(dmabuf_caps)
    }

    fn decide_allocation(
        &self,
        query: &mut gst::query::Allocation,
    ) -> Result<(), gst::LoggableError> {
        let (caps, _) = query.get_owned();
        let caps = caps.expect("query without caps");
        let video_info = gst_video::VideoInfo::from_caps(&caps).expect("failed to get video info");

        let settings = self.settings.lock().unwrap();

        let buffer_pool = SmithayBufferPool::new();
        let (allocator, params, align) = {
            gst::debug!(CAT, imp: self, "using gbm allocator");
            (
                GbmMemoryAllocator::new(
                    settings.render_node.clone().and_then(|n| n.dev_path()),
                    &video_info,
                )
                .upcast(),
                Some(gst::AllocationParams::new(
                    gst::MemoryFlags::empty(),
                    127,
                    0,
                    0,
                )),
                Some(gst_video::VideoAlignment::new(0, 0, 0, 0, &[31, 0, 0, 0])),
            )
        };

        if let Some((_, _, min, max)) = query.allocation_pools().get(0) {
            let mut config = buffer_pool.config();
            config.set_allocator(Some(&allocator), params.as_ref());
            config.add_option(gst_video::BUFFER_POOL_OPTION_VIDEO_META.as_ref());
            if let Some(video_align) = align.as_ref() {
                config.add_option(gst_video::BUFFER_POOL_OPTION_VIDEO_ALIGNMENT.as_ref());
                config.set_video_alignment(video_align);
            }
            let size = video_info.size() as u32;
            config.set_params(Some(&caps), size, *min, *max);
            buffer_pool
                .set_config(config)
                .expect("failed to set config");
            query.set_nth_allocation_pool(0, Some(&buffer_pool), size, *min, *max);
        } else {
            let mut config = buffer_pool.config();
            config.set_allocator(Some(&allocator), params.as_ref());
            config.add_option(gst_video::BUFFER_POOL_OPTION_VIDEO_META.as_ref());
            if let Some(video_align) = align.as_ref() {
                config.add_option(gst_video::BUFFER_POOL_OPTION_VIDEO_ALIGNMENT.as_ref());
                config.set_video_alignment(video_align);
            }
            let video_info =
                gst_video::VideoInfo::from_caps(&caps).expect("failed to get video info");
            config.set_params(Some(&caps), video_info.size() as u32, 0, 0);
            buffer_pool
                .set_config(config)
                .expect("failed to set config");
            query.add_allocation_pool(Some(&buffer_pool), video_info.size() as u32, 0, 0);
        };

        let _ = self
            .state
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .command_tx
            .send(Command::VideoInfo(video_info));

        Ok(())
    }

    fn set_caps(&self, caps: &gst::Caps) -> Result<(), gst::LoggableError> {
        self.parent_set_caps(caps)
    }

    fn start(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        if state.is_some() {
            return Ok(());
        }

        let settings = self.settings.lock().unwrap();
        let render_node = settings.render_node.clone().unwrap_or_else(|| {
            DrmNode::from_path("/dev/dri/renderD128")
                .expect("Failed to open default DRM render node")
        });
        let input_seat = settings
            .input_seat
            .clone()
            .unwrap_or_else(|| String::from("seat-0"));

        let (command_tx, command_src) = smithay::reexports::calloop::channel::channel();
        let thread_handle =
            std::thread::spawn(move || super::comp::init(command_src, render_node, &input_seat));

        *state = Some(State {
            thread_handle,
            command_tx,
        });

        Ok(())
    }

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        if let Some(state) = state.take() {
            if let Err(err) = state.command_tx.send(Command::Quit) {
                gst::warning!(CAT, "Failed to send stop command: {}", err);
                return Ok(());
            };
            std::mem::drop(state.command_tx);
            if state.thread_handle.join().is_err() {
                gst::warning!(CAT, "Failed to join compositor thread");
            };
        }

        Ok(())
    }

    fn is_seekable(&self) -> bool {
        false
    }
}

impl PushSrcImpl for WaylandDisplaySrc {
    fn create(&self, buffer: Option<&mut gst::BufferRef>) -> Result<CreateSuccess, gst::FlowError> {
        let mut state_guard = self.state.lock().unwrap();
        let Some(state) = state_guard.as_mut() else {
            return Err(gst::FlowError::Eos);
        };

        let Some(pool) = self.obj().buffer_pool() else {
            unreachable!()
        };

        let (buffer, dmabuf) = match buffer {
            Some(buffer_ref) => {
                let buffer_meta = buffer_ref
                    .meta::<SmithayBufferMeta>()
                    .expect("no smithay buffer meta");
                let dmabuf = buffer_meta.get_dma_buffer().clone();
                (None, dmabuf)
            }
            None => {
                let buffer_pool_aquire_params =
                    gst::BufferPoolAcquireParams::with_flags(gst::BufferPoolAcquireFlags::empty());
                let new_buffer = pool.acquire_buffer(Some(&buffer_pool_aquire_params))?;
                let buffer_meta = new_buffer
                    .meta::<SmithayBufferMeta>()
                    .expect("no smithay buffer meta");
                let dmabuf = buffer_meta.get_dma_buffer().clone();
                (Some(new_buffer), dmabuf)
            }
        };

        let (buffer_tx, buffer_rx) = mpsc::sync_channel(0);
        if let Err(err) = state.command_tx.send(Command::Buffer(dmabuf, buffer_tx)) {
            gst::warning!(CAT, "Failed to send buffer command: {}", err);
            return Err(gst::FlowError::Eos);
        }

        if let Err(err) = buffer_rx.recv() {
            gst::warning!(CAT, "Failed to recv buffer ack: {}", err);
            return Err(gst::FlowError::Error);
        }

        Ok(match buffer {
            Some(new_buffer) => CreateSuccess::NewBuffer(new_buffer),
            None => CreateSuccess::FilledBuffer,
        })
    }
}
