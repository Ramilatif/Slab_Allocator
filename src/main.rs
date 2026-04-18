#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use bootloader::{BootInfo, entry_point};

mod vga_buffer;
mod memory;
mod allocator;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    use x86_64::VirtAddr;
    use memory::BootInfoFrameAllocator;

    vga_buffer::init();
    println!("Kernel booting...");

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe {
        BootInfoFrameAllocator::init(&boot_info.memory_map)
    };

    allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");

    println!("Heap initialized!");
    println!("Slab allocator ready.");

    // Normal mode: run tests directly at boot for a quick sanity check.
    // Test mode (cargo test): test_main() collects them via #[test_case].
    #[cfg(not(test))]
    {
        test_small_alloc();
        test_many_boxes();
        test_vec();
        println!("All boot tests passed!");
        allocator::print_stats();
    }

    #[cfg(test)]
    test_main();

    loop {}
}

/// Allocates a single `Box<u64>` and checks its value.
///
/// Verifies that the slab allocator returns a valid pointer and that
/// the written value is correctly read back.
#[cfg_attr(test, test_case)]
fn test_small_alloc() {
    use alloc::boxed::Box;
    let x = Box::new(42u64);
    assert_eq!(*x, 42);
    println!("[ok] test_small_alloc");
}

/// Allocates 1 000 `Box<usize>` in a loop and checks each value.
///
/// After the first few allocations the slab cache is populated; subsequent
/// ones come from recycled objects via the freelist — this test validates
/// the full alloc → dealloc → reuse path.
#[cfg_attr(test, test_case)]
fn test_many_boxes() {
    use alloc::boxed::Box;
    for i in 0..1000usize {
        let x = Box::new(i);
        assert_eq!(*x, i);
    }
    println!("[ok] test_many_boxes");
}

/// Allocates a `Vec<u32>`, pushes 100 elements, and checks length and values.
///
/// Covers internal `Vec` reallocations (buffer growth), which exercises
/// multiple size classes of the slab allocator.
#[cfg_attr(test, test_case)]
fn test_vec() {
    use alloc::vec::Vec;
    let mut v: Vec<u32> = Vec::new();
    for i in 0..100u32 {
        v.push(i);
    }
    assert_eq!(v.len(), 100);
    assert_eq!(v[42], 42);
    println!("[ok] test_vec");
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
    println!("All tests passed!");
}