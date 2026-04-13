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

    // Basic smoke tests run at every boot.
    test_small_alloc();
    test_many_boxes();
    test_vec();
    println!("All boot tests passed!");

    allocator::print_stats();

    #[cfg(test)]
    test_main();

    loop {}
}

/// Allocates a single `Box<u64>` and checks its value.
fn test_small_alloc() {
    use alloc::boxed::Box;
    let x = Box::new(42u64);
    assert_eq!(*x, 42);
    println!("[ok] test_small_alloc");
}

/// Allocates 1 000 `Box<usize>` in a loop, checking each value.
///
/// This exercises the freelist: after the first few allocations the slab
/// cache is populated and subsequent allocs come from recycled objects.
fn test_many_boxes() {
    use alloc::boxed::Box;
    for i in 0..1000usize {
        let x = Box::new(i);
        assert_eq!(*x, i);
    }
    println!("[ok] test_many_boxes");
}

/// Allocates a `Vec` and pushes 100 elements to verify realloc behaviour.
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