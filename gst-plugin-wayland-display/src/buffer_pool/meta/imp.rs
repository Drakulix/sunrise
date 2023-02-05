use std::ptr;

use gst::glib::{
    self,
    translate::{from_glib, IntoGlib},
};

use once_cell::sync::Lazy;
use smithay::backend::allocator::dmabuf::Dmabuf;

pub(super) struct CustomMetaParams {
    pub dmabuf: Dmabuf,
}

#[repr(C)]
pub struct SmithayBufferMeta {
    parent: gst::ffi::GstMeta,
    pub(super) dmabuf: Dmabuf,
}

pub(super) fn custom_meta_api_get_type() -> glib::Type {
    static TYPE: Lazy<glib::Type> = Lazy::new(|| unsafe {
        let t = from_glib(gst::ffi::gst_meta_api_type_register(
            b"SmithayBufferMetaAPI\0".as_ptr() as *const _,
            [ptr::null::<std::os::raw::c_char>()].as_ptr() as *mut *const _,
        ));

        assert_ne!(t, glib::Type::INVALID);

        t
    });

    *TYPE
}

unsafe extern "C" fn custom_meta_init(
    meta: *mut gst::ffi::GstMeta,
    params: glib::ffi::gpointer,
    _buffer: *mut gst::ffi::GstBuffer,
) -> glib::ffi::gboolean {
    assert!(!params.is_null());

    let meta = &mut *(meta as *mut SmithayBufferMeta);
    let params = ptr::read(params as *const CustomMetaParams);

    // Need to initialize all our fields correctly here.
    ptr::write(&mut meta.dmabuf, params.dmabuf);

    true.into_glib()
}

// Free function for our meta. This needs to free/drop all memory we allocated.
unsafe extern "C" fn custom_meta_free(
    meta: *mut gst::ffi::GstMeta,
    _buffer: *mut gst::ffi::GstBuffer,
) {
    let meta = &mut *(meta as *mut SmithayBufferMeta);

    // Need to free/drop all our fields here.
    ptr::drop_in_place(&mut meta.dmabuf);
}

// Transform function for our meta. This needs to get it from the old buffer to the new one
// in a way that is compatible with the transformation type. In this case we just always
// copy it over.
unsafe extern "C" fn custom_meta_transform(
    dest: *mut gst::ffi::GstBuffer,
    meta: *mut gst::ffi::GstMeta,
    _buffer: *mut gst::ffi::GstBuffer,
    _type_: glib::ffi::GQuark,
    _data: glib::ffi::gpointer,
) -> glib::ffi::gboolean {
    let meta = &*(meta as *mut SmithayBufferMeta);

    // We simply copy over our meta here. Other metas might have to look at the type
    // and do things conditional on that, or even just drop the meta.
    super::SmithayBufferMeta::add(gst::BufferRef::from_mut_ptr(dest), meta.dmabuf.clone());

    true.into_glib()
}

// Register the meta itself with its functions.
pub(super) fn custom_meta_get_info() -> *const gst::ffi::GstMetaInfo {
    struct MetaInfo(ptr::NonNull<gst::ffi::GstMetaInfo>);
    unsafe impl Send for MetaInfo {}
    unsafe impl Sync for MetaInfo {}

    static META_INFO: Lazy<MetaInfo> = Lazy::new(|| unsafe {
        MetaInfo(
            ptr::NonNull::new(gst::ffi::gst_meta_register(
                custom_meta_api_get_type().into_glib(),
                b"DmabufMeta\0".as_ptr() as *const _,
                std::mem::size_of::<SmithayBufferMeta>(),
                Some(custom_meta_init),
                Some(custom_meta_free),
                Some(custom_meta_transform),
            ) as *mut gst::ffi::GstMetaInfo)
            .expect("Failed to register meta API"),
        )
    });

    META_INFO.0.as_ptr()
}
