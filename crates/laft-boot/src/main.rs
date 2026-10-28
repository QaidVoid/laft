//! `laft-boot`, the init of the Laft initramfs.
//!
//! It runs as PID 1 before the root filesystem is mounted. It mounts the
//! kernel filesystems, loads the configured kernel modules, waits for the
//! boot devices, unlocks LUKS devices, checks and mounts the root, chooses
//! and counts the generation to boot, mounts that generation, makes `/laft`
//! read-only and finally switches to the new root and executes the
//! generation's init. Every failure before the switch drops to an
//! emergency shell, after which the failed step is retried. A failed
//! switch reboots the machine.
//!
//! The configuration comes from `/laft-boot.json` in the initramfs and from
//! the `laft.*` parameters on the kernel command line.

#![forbid(unsafe_code)]

mod boot;
mod cmdline;
mod config;
mod counting;
mod device;
mod files;
mod generation;
mod log;
mod options;
mod plan;
mod probe;
mod programs;
mod shell;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    if !rustix::process::getpid().is_init() {
        log::console("must run as PID 1 in the initramfs");
        std::process::exit(1);
    }
    std::panic::set_hook(Box::new(shell::on_panic));
    boot::run()
}
