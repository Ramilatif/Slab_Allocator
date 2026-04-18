//! Host-testable slab allocator core.
//!
//! In `no_std` kernel builds this crate is a bare-metal library.
//! Under `cargo test` (host toolchain) `std` is available and the unit tests
//! below exercise the allocator logic directly — no QEMU required.
#![cfg_attr(not(test), no_std)]

/// Core slab allocator implementation.
///
/// Re-exports [`SlabAllocator`], [`SlabCache`], [`CacheStats`],
/// [`AllocatorStats`] and [`align_up`].
pub mod slab {
    include!("allocator/slab.rs");
}

// ── Host-native unit tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::slab::{align_up, SlabAllocator};
    use core::alloc::Layout;

    const HEAP_SIZE: usize = 65_536;

    /// Creates a fresh [`SlabAllocator`] backed by an independent heap buffer.
    ///
    /// The returned `Box<[u8]>` owns the backing memory and **must be kept
    /// alive** for the lifetime of the allocator (it is the second element of
    /// the tuple so it is dropped after the allocator).
    fn make_allocator() -> (SlabAllocator, Box<[u8]>) {
        let heap: Box<[u8]> = vec![0u8; HEAP_SIZE].into_boxed_slice();
        let start = heap.as_ptr() as usize;
        let mut alloc = SlabAllocator::new();
        unsafe { alloc.init(start, HEAP_SIZE) };
        (alloc, heap)
    }

    /// A single allocation followed by a deallocation updates stats correctly.
    #[test]
    fn test_alloc_dealloc() {
        let (mut alloc, _heap) = make_allocator();
        let layout = Layout::from_size_align(8, 8).unwrap();

        let ptr = alloc.alloc_test(layout);
        assert!(!ptr.is_null(), "alloc returned null");

        let stats = alloc.stats();
        assert_eq!(stats.caches[0].allocs, 1);
        assert_eq!(stats.caches[0].live, 1);

        alloc.dealloc_test(ptr, layout);

        let stats = alloc.stats();
        assert_eq!(stats.caches[0].deallocs, 1);
        assert_eq!(stats.caches[0].live, 0);
    }

    /// After 100 allocs + deallocs the live count is 0, and the next alloc is
    /// served from the freelist (total allocs == 101, no new bump space used).
    #[test]
    fn test_many_allocs_and_reuse() {
        let (mut alloc, _heap) = make_allocator();
        let layout = Layout::from_size_align(64, 64).unwrap();

        let ptrs: Vec<*mut u8> = (0..100).map(|_| alloc.alloc_test(layout)).collect();
        for &p in &ptrs {
            assert!(!p.is_null());
        }
        for &p in &ptrs {
            alloc.dealloc_test(p, layout);
        }
        assert_eq!(alloc.stats().caches[3].live, 0); // size-64 → cache index 3

        // Must be served from freelist, not a new slab.
        let ptr = alloc.alloc_test(layout);
        assert!(!ptr.is_null());
        assert_eq!(alloc.stats().caches[3].allocs, 101);
    }

    /// Requests larger than 2 048 B bypass the caches and go to the bump region.
    #[test]
    fn test_oversized_alloc() {
        let (mut alloc, _heap) = make_allocator();
        let layout = Layout::from_size_align(4096, 8).unwrap();

        let ptr = alloc.alloc_test(layout);
        assert!(!ptr.is_null());
        assert_eq!(alloc.stats().oversized_allocs, 1);
    }

    /// After `new()` every cache starts with `color_next == 0` and
    /// `color_step == max(64, object_size)`.
    #[test]
    fn test_slab_coloring_initial_state() {
        let alloc = SlabAllocator::new();
        for cache in &alloc.stats().caches {
            assert_eq!(cache.color_next, 0);
            assert_eq!(cache.color_step, 64_usize.max(cache.object_size));
        }
    }

    /// `align_up` rounds an address to the next multiple of `align`.
    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
        assert_eq!(align_up(0, 64), 0);
        assert_eq!(align_up(1, 64), 64);
    }
}
