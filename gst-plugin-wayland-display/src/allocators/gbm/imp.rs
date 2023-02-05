use std::os::fd::AsRawFd;
use std::os::unix::io::{AsFd, BorrowedFd};
use std::sync::Mutex;

use gst::glib;
use gst::prelude::{Cast, ParamSpecBuilderExt, ToValue};
use gst::subclass::prelude::*;
use gst_allocators::subclass::prelude::*;
use gst_allocators::DmaBufAllocator;
use once_cell::sync::Lazy;
use smithay::{
    backend::allocator::Fourcc,
    reexports::{gbm, nix::unistd},
};

/// A simple wrapper for a device node.
#[derive(Debug)]
pub struct Card(std::fs::File);

/// Implementing [`AsFd`] is a prerequisite to implementing the traits found
/// in this crate. Here, we are just calling [`File::as_fd()`] on the inner
/// [`File`].
impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

/// Simple helper methods for opening a `Card`.
impl Card {
    pub fn open(path: &str) -> Self {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        options.write(true);
        Card(options.open(path).unwrap())
    }
}

#[derive(Debug, Default)]
struct Settings {
    device_path: Option<String>,
    fourcc: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Default)]
pub struct GbmMemoryAllocator {
    settings: Mutex<Settings>,
    device: Mutex<Option<gbm::Device<Card>>>,
}

#[glib::object_subclass]
impl ObjectSubclass for GbmMemoryAllocator {
    const NAME: &'static str = "GbmMemoryAllocator";
    type Type = super::GbmMemoryAllocator;
    type ParentType = DmaBufAllocator;
    type Interfaces = ();
}

impl ObjectImpl for GbmMemoryAllocator {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecString::builder("device")
                    .nick("drm device")
                    .blurb("device path to allocator buffers from")
                    .construct()
                    .build(),
                glib::ParamSpecUInt::builder("fourcc")
                    .nick("video pixel format")
                    .blurb("pixel format to allocate gbm buffers in")
                    .construct()
                    .build(),
                glib::ParamSpecUInt::builder("width")
                    .nick("width")
                    .blurb("width of the buffer")
                    .construct()
                    .build(),
                glib::ParamSpecUInt::builder("height")
                    .nick("height")
                    .blurb("height of the buffer")
                    .construct()
                    .build(),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "device" => {
                let mut settings = self.settings.lock().unwrap();
                let device_path = value
                    .get::<Option<String>>()
                    .expect("type checked upstream");
                settings.device_path = device_path;
            }
            "fourcc" => {
                let mut settings = self.settings.lock().unwrap();
                let fourcc = value.get::<u32>().expect("type checked upstream");
                settings.fourcc = fourcc;
            }
            "width" => {
                let mut settings = self.settings.lock().unwrap();
                let width = value.get::<u32>().expect("type checked upstream");
                settings.width = width;
            }
            "height" => {
                let mut settings = self.settings.lock().unwrap();
                let height = value.get::<u32>().expect("type checked upstream");
                settings.height = height;
            }
            _ => unreachable!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "device" => {
                let settings = self.settings.lock().unwrap();
                settings.device_path.to_value()
            }
            "fourcc" => {
                let settings = self.settings.lock().unwrap();
                settings.fourcc.to_value()
            }
            "height" => {
                let settings = self.settings.lock().unwrap();
                settings.height.to_value()
            }
            "width" => {
                let settings = self.settings.lock().unwrap();
                settings.width.to_value()
            }
            _ => unreachable!(),
        }
    }

    fn constructed(&self) {
        let device_path = self
            .settings
            .lock()
            .unwrap()
            .device_path
            .clone()
            .unwrap_or_else(|| String::from("/dev/dri/renderD128"));
        *self.device.lock().unwrap() = Some(gbm::Device::new(Card::open(&device_path)).unwrap());
    }
}

impl GstObjectImpl for GbmMemoryAllocator {}

impl DmaBufAllocatorImpl for GbmMemoryAllocator {}
impl FdAllocatorImpl for GbmMemoryAllocator {}
impl AllocatorImpl for GbmMemoryAllocator {
    fn alloc(
        &self,
        size: usize,
        _params: Option<&gst::AllocationParams>,
    ) -> Result<gst::Memory, glib::BoolError> {
        let settings = self.settings.lock().unwrap();

        let obj = self.obj();
        let dmabuf_allocator: &DmaBufAllocator = obj.upcast_ref();

        let guard = self.device.lock().unwrap();
        let device = guard.as_ref().unwrap();

        let bo = device
            .create_buffer_object_with_modifiers2::<()>(
                settings.width,
                settings.height,
                Fourcc::try_from(settings.fourcc)
                    .expect("We choose this earlier, so we should know it"),
                [gbm::Modifier::Linear].into_iter(),
                gbm::BufferObjectFlags::RENDERING,
            )
            .expect("failed to create bo");
        let fd = bo.fd().expect("no fd");

        let fd_size = unistd::lseek(fd.as_raw_fd(), 0, unistd::Whence::SeekEnd).unwrap();
        let _ = unistd::lseek(fd.as_raw_fd(), 0, unistd::Whence::SeekSet);

        if (fd_size as usize) < size {
            panic!("bo too small");
        }

        let memory = unsafe {
            dmabuf_allocator
                .alloc(fd, fd_size as usize)
                .expect("failed to allocate dmabuf memory")
        };

        Ok(memory)
    }
}
