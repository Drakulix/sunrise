use std::path::Path;

use gst::glib;
use gst_video::VideoInfo;

use crate::utils::gst_video_format_to_drm_fourcc;

mod imp;

glib::wrapper! {
    pub struct GbmMemoryAllocator(ObjectSubclass<imp::GbmMemoryAllocator>) @extends gst_allocators::DmaBufAllocator, gst_allocators::FdAllocator, gst::Allocator, gst::Object;
}

impl GbmMemoryAllocator {
    pub fn new<P: AsRef<Path>>(device_path: Option<P>, info: &VideoInfo) -> Self {
        let device_path = device_path.map(|p| p.as_ref().to_str().unwrap().to_string());
        glib::Object::builder()
            .property("device", &device_path)
            .property(
                "fourcc",
                gst_video_format_to_drm_fourcc(info.format()).expect("We choose this") as u32,
            )
            .property("width", info.width())
            .property("height", info.height())
            .build()
    }
}
