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

// ── FreeNode ─────────────────────────────────────────────────────────────────

/// A node in a [`SlabCache`] freelist.
///
/// Stored **in-place** inside a free object so that the freelist itself
/// consumes no additional memory.
struct FreeNode {
    next: *mut FreeNode,
}

// ── SlabCache ─────────────────────────────────────────────────────────────────

/// A cache that manages a freelist of fixed-size objects for one size class.
///
/// Objects are carved out of the shared heap bump region on first use and
/// returned to the freelist on deallocation, ready for reuse.
pub struct SlabCache {
    /// Size of every object in this cache, in bytes.
    object_size: usize,
    /// Head of the intrusive freelist (`null` when empty).
    free_list: *mut FreeNode,
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
    pub const fn new(object_size: usize) -> Self {
        SlabCache { object_size, free_list: ptr::null_mut() }
    }

    /// Carves up to 4 096 bytes of objects from the bump region and adds them
    /// to the freelist.
    ///
    /// Does nothing if the bump region is exhausted.
    fn grow(&mut self, bump_next: &mut usize, bump_end: usize) {
        // Align start to object_size so every carved object is naturally aligned.
        let start = align_up(*bump_next, self.object_size);
        if start >= bump_end {
            return;
        }

        let available = (bump_end - start).min(4096);
        let count = available / self.object_size;
        if count == 0 {
            return;
        }

        *bump_next = start + count * self.object_size;

        for i in 0..count {
            let node = (start + i * self.object_size) as *mut FreeNode;
            // SAFETY: The memory range [start, *bump_next) was just reserved
            // from the bump region and is exclusively owned by this cache.
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
pub struct SlabAllocator {
    caches: [SlabCache; 9],
    bump_next: usize,
    bump_end: usize,
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
/// assert_eq!(slab_allocator::allocator::slab::align_up(5, 8), 8);
/// assert_eq!(slab_allocator::allocator::slab::align_up(16, 8), 16);
/// assert_eq!(slab_allocator::allocator::slab::align_up(0, 4), 0);
/// ```
pub fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}
