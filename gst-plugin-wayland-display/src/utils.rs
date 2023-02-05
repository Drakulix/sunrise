use gst_video::VideoFormat;
use smithay::backend::allocator::Fourcc;

pub fn gst_video_format_from_drm_fourcc(format: Fourcc) -> Option<VideoFormat> {
    let format = match format {
        Fourcc::Abgr8888 => VideoFormat::Rgba,
        Fourcc::Argb8888 => VideoFormat::Bgra,
        Fourcc::Bgra8888 => VideoFormat::Argb,
        Fourcc::Bgrx8888 => VideoFormat::Xrgb,
        Fourcc::Rgba8888 => VideoFormat::Abgr,
        Fourcc::Rgbx8888 => VideoFormat::Xbgr,
        Fourcc::Xbgr8888 => VideoFormat::Rgbx,
        Fourcc::Xrgb8888 => VideoFormat::Bgrx,
        _ => return None,
    };
    Some(format)
}

pub fn gst_video_format_to_drm_fourcc(format: VideoFormat) -> Option<Fourcc> {
    let format = match format {
        VideoFormat::Abgr => Fourcc::Rgba8888,
        VideoFormat::Argb => Fourcc::Bgra8888,
        VideoFormat::Bgra => Fourcc::Argb8888,
        VideoFormat::Bgrx => Fourcc::Xrgb8888,
        VideoFormat::Rgba => Fourcc::Abgr8888,
        VideoFormat::Rgbx => Fourcc::Xbgr8888,
        VideoFormat::Xbgr => Fourcc::Rgbx8888,
        VideoFormat::Xrgb => Fourcc::Bgrx8888,
        _ => return None,
    };
    Some(format)
}
