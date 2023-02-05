use gst::{glib, MetaAPI};
use smithay::backend::allocator::dmabuf::Dmabuf;
mod imp;

#[repr(transparent)]
pub struct SmithayBufferMeta(imp::SmithayBufferMeta);

unsafe impl Send for SmithayBufferMeta {}
unsafe impl Sync for SmithayBufferMeta {}

impl SmithayBufferMeta {
    // Add a new custom meta to the buffer with the given label.
    pub fn add(
        buffer: &mut gst::BufferRef,
        dmabuf: Dmabuf,
    ) -> gst::MetaRefMut<Self, gst::meta::Standalone> {
        unsafe {
            // Manually dropping because gst_buffer_add_meta() takes ownership of the
            // content of the struct.
            let mut params = std::mem::ManuallyDrop::new(imp::CustomMetaParams { dmabuf });

            // The label is passed through via the params to custom_meta_init().
            let meta = gst::ffi::gst_buffer_add_meta(
                buffer.as_mut_ptr(),
                imp::custom_meta_get_info(),
                &mut *params as *mut imp::CustomMetaParams as glib::ffi::gpointer,
            ) as *mut imp::SmithayBufferMeta;

            Self::from_mut_ptr(buffer, meta)
        }
    }

    // Retrieve the stored [`Dmabuf`].
    pub fn get_dma_buffer(&self) -> &Dmabuf {
        &self.0.dmabuf
    }
}

// Trait to allow using the gst::Buffer API with this meta.
unsafe impl MetaAPI for SmithayBufferMeta {
    type GstType = imp::SmithayBufferMeta;

    fn meta_api() -> glib::Type {
        imp::custom_meta_api_get_type()
    }
}

impl std::fmt::Debug for SmithayBufferMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("SmithayBufferMeta")
            .field("dmabuf", &self.0.dmabuf)
            .finish()
    }
}
