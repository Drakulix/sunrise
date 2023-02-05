use gstreamer::glib;

mod imp;

glib::wrapper! {
    pub struct DmaHeapMemoryAllocator(ObjectSubclass<imp::DmaHeapMemoryAllocator>) @extends gstreamer_allocators::DmaBufAllocator, gstreamer_allocators::FdAllocator, gstreamer::Allocator, gstreamer::Object;
}

impl DmaHeapMemoryAllocator {
    pub fn is_available() -> bool {
        imp::DmaHeapMemoryAllocator::is_available()
    }
}

impl Default for DmaHeapMemoryAllocator {
    fn default() -> Self {
        glib::Object::new(&[])
    }
}
