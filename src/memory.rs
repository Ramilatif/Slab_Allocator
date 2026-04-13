//! Kernel memory management: page table initialisation and heap mapping.
//!
//! Provides two building blocks used at boot time:
//!
//! 1. [`init`] — reads the CR3 register and returns an [`OffsetPageTable`]
//!    that maps physical addresses to virtual ones using the fixed offset
//!    provided by the bootloader.
//! 2. [`BootInfoFrameAllocator`] — a simple physical-frame allocator driven
//!    by the bootloader's memory map.
//!
//! The heap region is defined by [`HEAP_START`] and [`HEAP_SIZE`] and is
//! mapped into the virtual address space by [`init_heap`].

use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page,
        PageTableFlags, PhysFrame, Size4KiB, OffsetPageTable,
    },
    PhysAddr, VirtAddr,
};
use bootloader::bootinfo::{MemoryMap, MemoryRegionType};

/// Virtual start address of the kernel heap.
///
/// Chosen to be far from both the identity-mapped physical memory region
/// and the kernel binary itself.
pub const HEAP_START: usize = 0x_4444_4444_0000;

/// Size of the kernel heap in bytes (1 MiB).
pub const HEAP_SIZE: usize = 1024 * 1024;

// ── Page table ───────────────────────────────────────────────────────────────

/// Initialises and returns the active level-4 [`OffsetPageTable`].
///
/// Reads the physical address of the L4 page table from the `CR3` register,
/// converts it to a virtual address using `physical_memory_offset`, and wraps
/// it in an [`OffsetPageTable`] that can map/unmap pages.
///
/// # Safety
/// The caller must guarantee that:
/// - `physical_memory_offset` is the correct offset at which the entire
///   physical memory is mapped in the virtual address space.
/// - This function is called at most once (aliasing the L4 table is UB).
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    use x86_64::registers::control::Cr3;
    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr = virt.as_mut_ptr();
    OffsetPageTable::new(&mut *page_table_ptr, physical_memory_offset)
}

// ── Frame allocator ──────────────────────────────────────────────────────────

/// A physical-frame allocator backed by the bootloader's memory map.
///
/// Iterates over all memory regions marked [`Usable`](MemoryRegionType::Usable)
/// and hands them out one 4 KiB frame at a time. Frames are never reused —
/// this is intentional for boot-time allocation where simplicity matters more
/// than recycling.
pub struct BootInfoFrameAllocator {
    /// Reference to the memory map provided by the bootloader.
    memory_map: &'static MemoryMap,
    /// Index of the next frame to hand out (monotonically increasing).
    next: usize,
}

impl BootInfoFrameAllocator {
    /// Creates a new `BootInfoFrameAllocator` from the bootloader memory map.
    ///
    /// # Safety
    /// The caller must guarantee that `memory_map` is valid and that all
    /// frames marked as [`Usable`](MemoryRegionType::Usable) are genuinely
    /// unused physical memory.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        BootInfoFrameAllocator { memory_map, next: 0 }
    }

    /// Returns an iterator over all usable 4 KiB physical frames.
    ///
    /// Filters the memory map to keep only usable regions, then produces
    /// one [`PhysFrame`] per 4 096-byte step within each region.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        self.memory_map.iter()
            .filter(|r| r.region_type == MemoryRegionType::Usable)
            .map(|r| r.range.start_addr()..r.range.end_addr())
            .flat_map(|r| r.step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    /// Allocates the next available 4 KiB physical frame.
    ///
    /// Returns `None` if the usable memory region is exhausted.
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

// ── Heap mapping ─────────────────────────────────────────────────────────────

/// Maps the kernel heap pages into the virtual address space.
///
/// Iterates over every 4 KiB page in `[HEAP_START, HEAP_START + HEAP_SIZE)`
/// and maps each one to a freshly allocated physical frame with
/// `PRESENT | WRITABLE` flags. After this call the address range is safe
/// to use as a heap.
///
/// # Errors
/// Returns [`MapToError::FrameAllocationFailed`] if the frame allocator runs
/// out of physical memory, or a page-table error if a page is already mapped.
///
/// # Examples
///
/// ```ignore
/// // Typical boot-time usage:
/// let mut mapper = unsafe { memory::init(phys_mem_offset) };
/// let mut frame_alloc = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
/// memory::init_heap(&mut mapper, &mut frame_alloc).expect("heap init failed");
/// ```
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE - 1u64;
        let start_page = Page::containing_address(heap_start);
        let end_page = Page::containing_address(heap_end);
        Page::range_inclusive(start_page, end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush()
        };
    }
    Ok(())
}
