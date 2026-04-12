use x86_64::structures::paging::{
    mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags,
    PhysFrame, Size4KiB,
};
use x86_64::VirtAddr;
use crate::memory::{HEAP_START, HEAP_SIZE};

pub mod slab;

use slab::SlabAllocator;
use spin::Mutex;

#[global_allocator]
static ALLOCATOR: Mutex<SlabAllocator> = Mutex::new(SlabAllocator::new());

pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    crate::memory::init_heap(mapper, frame_allocator)?;
    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }
    Ok(())
}