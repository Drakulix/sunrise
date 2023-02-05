use gst::glib;

mod imp;
mod meta;

pub use meta::SmithayBufferMeta;

glib::wrapper! {
    pub struct SmithayBufferPool(ObjectSubclass<imp::SmithayBufferPool>) @extends gst::BufferPool, gst::Object;
}

impl SmithayBufferPool {
    pub fn new() -> Self {
        glib::Object::new()
    }
}
