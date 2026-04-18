# Slab Allocator — Bare-metal Rust Kernel

A `no_std` x86_64 kernel implementing a slab allocator inspired by the Linux SLUB allocator.  
Built with Rust nightly, `bootloader 0.9`, and QEMU.

---

## Features

- **9 size classes** — 8 B → 2 048 B (powers of two)
- **Intrusive freelist** — free objects store the next pointer in-place (zero overhead)
- **Slab coloring** — successive slabs are offset by `max(64, object_size)` bytes to reduce L1 cache conflicts
- **Allocation statistics** — per-cache allocs / deallocs / live count + bump region usage
- **Oversized fallback** — allocations > 2 048 B are served directly from the bump region
- **Host-native unit tests** — `cargo test` runs on the host (no QEMU needed)

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust nightly | latest | `rustup toolchain install nightly` |
| rust-src component | — | `rustup component add rust-src --toolchain nightly` |
| bootimage | 0.10 | `cargo install bootimage` |
| QEMU (x86_64) | any | [qemu.org](https://www.qemu.org/download/) |

> The `rust-toolchain.toml` at the root pins the nightly channel automatically.

---

## Commands

### Run the unit tests (host, no QEMU)

```powershell
cargo test
```

Compiles the slab allocator as a host library and runs 5 unit tests natively.  
No QEMU, no bootimage — instant feedback.

```
running 5 tests
test tests::test_align_up ... ok
test tests::test_alloc_dealloc ... ok
test tests::test_many_allocs_and_reuse ... ok
test tests::test_oversized_alloc ... ok
test tests::test_slab_coloring_initial_state ... ok

test result: ok. 5 passed; 0 failed; 0 ignored
```

---

### Boot the kernel in QEMU

```powershell
cargo krun
```

Builds the bare-metal kernel with `build-std` for the custom `x86_64-Slab_Allocator` target,  
creates a bootable image, and launches QEMU.

At boot you should see:

```
Kernel booting...
Heap initialized!
Slab allocator ready.
[ok] test_small_alloc
[ok] test_many_boxes
[ok] test_vec
All boot tests passed!
=== Slab Allocator Statistics ===
  size   allocs    frees    live  color/step
---------------------------------
    8B        1        1       0     64B/64B
   ...
```

Press **Ctrl-C** (or close the QEMU window) to exit.

---

### Build the kernel without running

```powershell
cargo kbuild
```

---

### Generate the documentation

```powershell
cargo doc --open
```

> For the kernel docs (with `build-std`):
> ```powershell
> cargo doc --target x86_64-Slab_Allocator.json -Z build-std=core,compiler_builtins,alloc
> ```

---

## Why `cargo krun` instead of `cargo run`?

`cargo run` targets the **host** by default (Windows/Linux). Since the kernel is `no_std` with no standard entry point, the host linker fails.

`cargo krun` is a Cargo alias defined in `.cargo/config.toml` that injects the right flags:

```
cargo krun
  ↓
cargo run -Z build-std=core,compiler_builtins,alloc \
          -Z build-std-features=compiler-builtins-mem \
          --target x86_64-Slab_Allocator.json
```

This separation also keeps `cargo test` clean — `build-std` is **not** in the global config, so the host toolchain compiles the tests without the duplicate-`core` issue.

---

## Project Structure

```
Slab_Allocator/
├── src/
│   ├── lib.rs                  # Host-testable lib: includes slab.rs + unit tests
│   ├── main.rs                 # Kernel entry point (no_std, no_main)
│   ├── memory.rs               # Page table init + BootInfoFrameAllocator + heap mapping
│   ├── vga_buffer.rs           # VGA text-mode driver (volatile writes, Writer)
│   └── allocator/
│       ├── mod.rs              # Locked<A> wrapper, GlobalAlloc impl, print_stats
│       └── slab.rs             # SlabAllocator, SlabCache, slab coloring, statistics
├── .cargo/
│   └── config.toml             # json-target-spec, krun/kbuild aliases, QEMU runner
├── x86_64-Slab_Allocator.json  # Custom bare-metal target specification
├── rust-toolchain.toml         # Pins nightly toolchain
└── Cargo.toml                  # [lib] (tests) + [[bin]] (kernel)
```

---

## Size Classes

| Index | Size | Color step |
|-------|------|-----------|
| 0 | 8 B | 64 B |
| 1 | 16 B | 64 B |
| 2 | 32 B | 64 B |
| 3 | 64 B | 64 B |
| 4 | 128 B | 128 B |
| 5 | 256 B | 256 B |
| 6 | 512 B | 512 B |
| 7 | 1 024 B | 1 024 B |
| 8 | 2 048 B | 2 048 B |

Objects larger than 2 048 B are served directly from the bump region (no freelist).
