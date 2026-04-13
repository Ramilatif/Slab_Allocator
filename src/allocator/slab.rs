/// Slab allocator inspired by the Linux SLUB allocator.
///
/// Maintains one [`SlabCache`] per size class. Each cache holds an intrusive
/// freelist of previously freed objects. When a cache is empty it carves new
/// objects out of a shared bump region. Allocations larger than the largest
/// size class fall back to the bump region directly (those are never returned
/// to a freelist).
///
/// # Size classes
///
/// | Index | Object size |
/// |-------|------------|
/// | 0     | 8 B        |
/// | 1     | 16 B       |
/// | 2     | 32 B       |
/// | 3     | 64 B       |
/// | 4     | 128 B      |
/// | 5     | 256 B      |
/// | 6     | 512 B      |
/// | 7     | 1 024 B    |
/// | 8     | 2 048 B    |

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

/// The supported size classes in bytes (must be powers of two, ascending).
const SLAB_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048];

/// L1 cache line size on x86_64 (bytes).
const CACHE_LINE_SIZE: usize = 64;

/// Number of distinct color offsets cycled per cache.
const NUM_COLORS: usize = 4;

// ── FreeNode ─────────────────────────────────────────────────────────────────

/// A node in a [`SlabCache`] freelist.
///
/// Stored **in-place** inside a free object so that the freelist itself
/// consumes no additional memory.
struct FreeNode {
    next: *mut FreeNode,
}

// ── SlabCache ─────────────────────────────────────────────────────────────────

// ── CacheStats ────────────────────────────────────────────────────────────────

/// Snapshot of allocation statistics for one [`SlabCache`].
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    /// Size class this cache serves, in bytes.
    pub object_size: usize,
    /// Total number of successful allocations from this cache.
    pub allocs: usize,
    /// Total number of deallocations returned to this cache.
    pub deallocs: usize,
    /// Currently live objects (`allocs - deallocs`).
    pub live: usize,
    /// Current slab color offset in bytes.
    pub color_next: usize,
    /// Color step in bytes (`max(CACHE_LINE_SIZE, object_size)`).
    pub color_step: usize,
}

// ── SlabCache ─────────────────────────────────────────────────────────────────

/// A cache that manages a freelist of fixed-size objects for one size class.
///
/// Objects are carved out of the shared heap bump region on first use and
/// returned to the freelist on deallocation, ready for reuse.
/// A cache that manages a freelist of fixed-size objects for one size class,
/// with slab coloring to reduce CPU cache line conflicts.
///
/// **Slab coloring**: each time a new slab is carved from the bump region, the
/// start address is shifted by `color_next` bytes. `color_next` advances by
/// `color_step = max(CACHE_LINE_SIZE, object_size)` and wraps after
/// `NUM_COLORS` steps. This spreads objects across different cache lines so
/// that simultaneous accesses to different caches do not compete for the same
/// L1 cache sets.
pub struct SlabCache {
    /// Size of every object in this cache, in bytes.
    object_size: usize,
    /// Head of the intrusive freelist (`null` when empty).
    free_list: *mut FreeNode,
    /// Total allocations served by this cache.
    allocs: usize,
    /// Total deallocations returned to this cache.
    deallocs: usize,
    /// Current color offset applied to the next slab (bytes).
    color_next: usize,
    /// Increment per color step: `max(CACHE_LINE_SIZE, object_size)`.
    color_step: usize,
    /// Maximum color offset before wrapping (`color_step * NUM_COLORS`).
    color_max: usize,
}

impl SlabCache {
    /// Creates a new, empty `SlabCache` for objects of `object_size` bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// # use slab_allocator::allocator::slab::SlabCache;
    /// let cache = SlabCache::new(64);
    /// ```
    /// Creates a new, empty `SlabCache` for objects of `object_size` bytes.
    ///
    /// The color step is `max(CACHE_LINE_SIZE, object_size)` so that the color
    /// offset is always a multiple of `object_size` (preserving alignment) and
    /// at least one full cache line apart (ensuring cache separation).
    pub const fn new(object_size: usize) -> Self {
        // const-friendly max: both are powers of two
        let color_step = if object_size >= CACHE_LINE_SIZE { object_size } else { CACHE_LINE_SIZE };
        SlabCache {
            object_size,
            free_list: ptr::null_mut(),
            allocs: 0,
            deallocs: 0,
            color_next: 0,
            color_step,
            color_max: color_step * NUM_COLORS,
        }
    }

    /// Returns a statistics snapshot for this cache.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            object_size: self.object_size,
            allocs: self.allocs,
            deallocs: self.deallocs,
            live: self.allocs - self.deallocs,
            color_next: self.color_next,
            color_step: self.color_step,
        }
    }

    /// Carves up to 4 096 bytes of objects from the bump region and adds them
    /// to the freelist.
    ///
    /// Does nothing if the bump region is exhausted.
    /// Carves up to 4 096 bytes of objects from the bump region and adds them
    /// to the freelist, applying the current slab color offset.
    ///
    /// The color offset shifts the slab start by `color_next` bytes so that
    /// successive slabs land on different L1 cache sets. `color_next` advances
    /// by `color_step` and wraps at `color_max`.
    fn grow(&mut self, bump_next: &mut usize, bump_end: usize) {
        // Align base to object_size, then apply color offset.
        // color_step is a multiple of object_size, so colored_start stays aligned.
        let base = align_up(*bump_next, self.object_size);
        let colored_start = base + self.color_next;

        // Advance the color for the next slab now, before the early returns.
        self.color_next += self.color_step;
        if self.color_next >= self.color_max {
            self.color_next = 0;
        }

        if colored_start >= bump_end {
            return;
        }

        let available = (bump_end - colored_start).min(4096);
        let count = available / self.object_size;
        if count == 0 {
            return;
        }

        *bump_next = colored_start + count * self.object_size;

        for i in 0..count {
            let node = (colored_start + i * self.object_size) as *mut FreeNode;
            // SAFETY: [colored_start, *bump_next) was just reserved from the
            // bump region and is exclusively owned by this cache.
            unsafe {
                (*node).next = self.free_list;
                self.free_list = node;
            }
        }
    }

    /// Allocates one object from this cache.
    ///
    /// Calls [`grow`](SlabCache::grow) if the freelist is empty.
    /// Returns a null pointer on out-of-memory.
    ///
    /// # Safety
    /// The caller must not use `bump_next` / `bump_end` for anything else
    /// while this call is in progress.
    fn alloc(&mut self, bump_next: &mut usize, bump_end: usize) -> *mut u8 {
        if self.free_list.is_null() {
            self.grow(bump_next, bump_end);
        }
        if self.free_list.is_null() {
            return ptr::null_mut();
        }
        // Pop from freelist.
        let node = self.free_list;
        // SAFETY: `node` was placed on the freelist by `grow` or `dealloc`,
        // so the pointer is valid and properly aligned.
        unsafe {
            self.free_list = (*node).next;
        }
        self.allocs += 1;
        node as *mut u8
    }

    /// Returns an object to the freelist so it can be reused.
    ///
    /// # Safety
    /// `ptr` must have been returned by [`alloc`](SlabCache::alloc) on this
    /// same cache and must not be used after this call.
    ///
    /// # Panics
    /// Does not panic; incorrect pointers cause silent undefined behaviour
    /// (by design, matching the `GlobalAlloc` contract).
    fn dealloc(&mut self, ptr: *mut u8) {
        let node = ptr as *mut FreeNode;
        // SAFETY: caller guarantees `ptr` is a live allocation from this cache.
        unsafe {
            (*node).next = self.free_list;
            self.free_list = node;
        }
        self.deallocs += 1;
    }
}

// SAFETY: All access to SlabAllocator and SlabCache is serialized through the
// spin::Mutex in Locked<SlabAllocator>. The raw pointers in the freelists are
// never accessed concurrently.
unsafe impl Send for SlabAllocator {}
unsafe impl Sync for SlabAllocator {}

// ── SlabAllocator ─────────────────────────────────────────────────────────────

/// A multi-size-class slab allocator backed by a contiguous heap region.
///
/// One [`SlabCache`] exists per size class in [`SLAB_SIZES`]. Allocations
/// larger than 2 048 bytes are served by a simple bump allocator and are
/// never returned to a cache on deallocation.
///
/// # Usage
///
/// ```ignore
/// // Typically registered as the global allocator via Locked<SlabAllocator>.
/// let mut alloc = SlabAllocator::new();
/// unsafe { alloc.init(HEAP_START, HEAP_SIZE); }
/// ```
/// Global statistics snapshot returned by [`SlabAllocator::stats`].
#[derive(Debug, Clone)]
pub struct AllocatorStats {
    /// Per-cache statistics, one entry per size class.
    pub caches: [CacheStats; 9],
    /// Number of allocations served directly from the bump region (size > 2 048 B).
    pub oversized_allocs: usize,
    /// Bytes still available in the bump region.
    pub bump_free_bytes: usize,
}

pub struct SlabAllocator {
    caches: [SlabCache; 9],
    bump_next: usize,
    bump_end: usize,
    /// Oversized allocations served by the bump region.
    oversized_allocs: usize,
}

impl SlabAllocator {
    /// Creates a new, uninitialized `SlabAllocator`.
    ///
    /// You **must** call [`init`](SlabAllocator::init) before any allocation.
    pub const fn new() -> Self {
        SlabAllocator {
            caches: [
                SlabCache::new(8),
                SlabCache::new(16),
                SlabCache::new(32),
                SlabCache::new(64),
                SlabCache::new(128),
                SlabCache::new(256),
                SlabCache::new(512),
                SlabCache::new(1024),
                SlabCache::new(2048),
            ],
            bump_next: 0,
            bump_end: 0,
            oversized_allocs: 0,
        }
    }

    /// Returns a snapshot of allocator statistics.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let stats = allocator.stats();
    /// assert_eq!(stats.caches[3].object_size, 64);
    /// ```
    pub fn stats(&self) -> AllocatorStats {
        AllocatorStats {
            caches: core::array::from_fn(|i| self.caches[i].stats()),
            oversized_allocs: self.oversized_allocs,
            bump_free_bytes: self.bump_end.saturating_sub(self.bump_next),
        }
    }

    /// Initializes the allocator with the heap region `[start, start + size)`.
    ///
    /// # Safety
    /// The entire range must be valid, writable, mapped memory not used by
    /// anything else.
    pub unsafe fn init(&mut self, start: usize, size: usize) {
        self.bump_next = start;
        self.bump_end = start + size;
    }

    /// Returns the index of the smallest size class that fits `size` bytes,
    /// or `None` if `size` exceeds all classes.
    fn cache_index(size: usize) -> Option<usize> {
        SLAB_SIZES.iter().position(|&s| s >= size)
    }
}

unsafe impl GlobalAlloc for SlabAllocator {
    /// Allocates a memory block satisfying `layout`.
    ///
    /// Finds the smallest fitting size class and pops from its freelist.
    /// Falls back to the bump region for oversized requests.
    /// Returns a null pointer on out-of-memory.
    ///
    /// # Safety
    /// Must be called through a `Mutex` (e.g. [`Locked`](super::Locked)) to
    /// ensure exclusive access, since interior mutation is performed via a raw
    /// pointer cast.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: exclusive access is guaranteed by the surrounding Mutex.
        let this = self as *const Self as *mut Self;

        // The object must be large enough to store a FreeNode when freed.
        let size = layout
            .size()
            .max(layout.align())
            .max(core::mem::size_of::<FreeNode>());

        match SlabAllocator::cache_index(size) {
            Some(i) => {
                (*this).caches[i].alloc(&mut (*this).bump_next, (*this).bump_end)
            }
            None => {
                // Oversized: serve directly from bump region.
                let start = align_up((*this).bump_next, layout.align());
                let end = match start.checked_add(layout.size()) {
                    Some(e) => e,
                    None => return ptr::null_mut(),
                };
                if end > (*this).bump_end {
                    return ptr::null_mut();
                }
                (*this).bump_next = end;
                (*this).oversized_allocs += 1;
                start as *mut u8
            }
        }
    }

    /// Deallocates the block pointed to by `ptr`.
    ///
    /// Returns the block to its size class freelist for future reuse.
    /// Oversized allocations (served from the bump region) are silently
    /// ignored — the bump region does not support individual frees.
    ///
    /// # Safety
    /// `ptr` must have been returned by [`alloc`] with the same `layout`.
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: exclusive access is guaranteed by the surrounding Mutex.
        let this = self as *const Self as *mut Self;

        let size = layout
            .size()
            .max(layout.align())
            .max(core::mem::size_of::<FreeNode>());

        if let Some(i) = SlabAllocator::cache_index(size) {
            (*this).caches[i].dealloc(ptr);
        }
        // Oversized bump allocations: no-op (memory is permanently consumed).
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Rounds `addr` up to the next multiple of `align`.
///
/// `align` must be a power of two.
///
/// # Examples
///
/// ```
/// # fn align_up(addr: usize, align: usize) -> usize {
/// #     (addr + align - 1) & !(align - 1)
/// # }
/// assert_eq!(align_up(5, 8), 8);   // rounds up
/// assert_eq!(align_up(16, 8), 16); // already aligned, unchanged
/// assert_eq!(align_up(0, 4), 0);   // zero stays zero
/// ```
pub fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}
