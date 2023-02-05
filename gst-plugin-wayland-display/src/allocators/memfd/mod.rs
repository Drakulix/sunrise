use gst::glib;

mod imp;

glib::wrapper! {
    pub struct MemfdMemoryAllocator(ObjectSubclass<imp::MemfdMemoryAllocator>) @extends gst_allocators::FdAllocator, gst::Allocator, gst::Object;
}

impl Default for MemfdMemoryAllocator {
    fn default() -> Self {
        glib::Object::new(&[])
    }
}
