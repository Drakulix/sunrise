use std::os::unix::io::IntoRawFd;

use gstreamer::glib;
use gstreamer::prelude::Cast;
use gstreamer::subclass::prelude::*;
use gstreamer_allocators::DmaBufAllocator;

#[derive(Debug)]
pub struct DmaHeapMemoryAllocator {
    heap: dma_heap::Heap,
}

impl DmaHeapMemoryAllocator {
    pub fn is_available() -> bool {
        dma_heap::Heap::new(dma_heap::HeapKind::Cma)
            .or_else(|_| dma_heap::Heap::new(dma_heap::HeapKind::System))
            .is_ok()
    }
}

impl Default for DmaHeapMemoryAllocator {
    fn default() -> Self {
        Self {
            heap: dma_heap::Heap::new(dma_heap::HeapKind::Cma)
                .unwrap_or_else(|_| dma_heap::Heap::new(dma_heap::HeapKind::System).unwrap()),
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for DmaHeapMemoryAllocator {
    const NAME: &'static str = "DmaHeapMemoryAllocator";
    type Type = super::DmaHeapMemoryAllocator;
    type ParentType = DmaBufAllocator;
    type Interfaces = ();
}

impl ObjectImpl for DmaHeapMemoryAllocator {}

impl GstObjectImpl for DmaHeapMemoryAllocator {}

impl AllocatorImpl for DmaHeapMemoryAllocator {
    fn alloc(
        &self,
        size: usize,
        _params: Option<&gstreamer::AllocationParams>,
    ) -> Result<gstreamer::Memory, glib::BoolError> {
        let obj = self.obj();
        let dmabuf_allocator: &DmaBufAllocator = obj.upcast_ref();

        let fd = self.heap.allocate(size).unwrap();
        unsafe { dmabuf_allocator.alloc(fd.into_raw_fd(), size) }
    }

    fn free(&self, memory: gstreamer::Memory) {
        self.parent_free(memory)
    }
}
