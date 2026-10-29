//! The `laft-boot` executable, the init of the Laft initramfs. Everything
//! but the check that it runs as PID 1 lives in the library.

#![forbid(unsafe_code)]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    if !rustix::process::getpid().is_init() {
        laft_boot::console("must run as PID 1 in the initramfs");
        std::process::exit(1);
    }
    laft_boot::run()
}
