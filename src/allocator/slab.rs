/// Temporary bump allocator stub — will be replaced by the real slab allocator.
///
/// Allocates linearly from the heap, never frees memory. Used only to validate
/// that the kernel boots and the heap is correctly mapped.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A simple bump allocator used as a placeholder until the slab allocator is implemented.
pub struct SlabAllocator {
    heap_start: usize,
    heap_end: usize,
    next: AtomicUsize,
}

impl SlabAllocator {
    /// Creates a new uninitialized `SlabAllocator`.
    pub const fn new() -> Self {
        SlabAllocator {
            heap_start: 0,
            heap_end: 0,
            next: AtomicUsize::new(0),
        }
    }

    /// Initializes the allocator with the given heap region.
    ///
    /// # Safety
    /// The caller must guarantee that `[start, start + size)` is valid, mapped,
    /// writable memory not used by anything else.
    pub unsafe fn init(&mut self, start: usize, size: usize) {
        self.heap_start = start;
        self.heap_end = start + size;
        self.next.store(start, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for SlabAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        loop {
            let current = self.next.load(Ordering::Relaxed);
            let aligned = align_up(current, layout.align());
            let next = aligned + layout.size();

            if next > self.heap_end {
                return ptr::null_mut();
            }

            if self.next.compare_exchange(
                current, next,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ).is_ok() {
                return aligned as *mut u8;
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator does not free memory.
    }
}

/// Aligns `addr` upward to the nearest multiple of `align`.
///
/// `align` must be a power of two.
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}
