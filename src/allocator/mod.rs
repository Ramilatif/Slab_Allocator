use x86_64::structures::paging::{mapper::MapToError, FrameAllocator, Mapper, Size4KiB};
use crate::memory::{HEAP_START, HEAP_SIZE};
use core::alloc::{GlobalAlloc, Layout};
use spin::Mutex;

pub mod slab;
use slab::SlabAllocator;

/// A wrapper that provides interior mutability for a `GlobalAlloc` implementor.
///
/// Since `GlobalAlloc::alloc` takes `&self`, we need a `Mutex` to safely mutate
/// the inner allocator state through a shared reference.
pub struct Locked<A> {
    inner: Mutex<A>,
}

impl<A> Locked<A> {
    /// Creates a new `Locked` wrapper around `inner`.
    pub const fn new(inner: A) -> Self {
        Locked { inner: Mutex::new(inner) }
    }
}

unsafe impl GlobalAlloc for Locked<SlabAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner.lock().dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Locked<SlabAllocator> = Locked::new(SlabAllocator::new());

/// Maps heap pages and initializes the slab allocator.
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    crate::memory::init_heap(mapper, frame_allocator)?;
    unsafe {
        ALLOCATOR.inner.lock().init(HEAP_START, HEAP_SIZE);
    }
    Ok(())
}
