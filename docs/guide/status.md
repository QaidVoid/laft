# Status and roadmap

<p class="lead">Laft is pre-release software, and no version of it has been released yet. It installs and boots in a virtual machine, its tools are covered by unit, integration and namespace tests, and its base system bootstraps from source. It is not ready for real hardware or for daily use, and nothing about its formats or commands is stable yet.</p>

## What works today

<span class="works">works</span> **Installing and booting.** `laft install` installs onto an empty ext4 root and a FAT32 EFI system partition. The machine boots under UEFI through Limine and `laft-boot` into runit, with every early service, a getty and the configured services. The [virtual machine test](./try-it) does this on every run, then boots a second time and checks that the first boot was recorded as good.

<span class="works">works</span> **Generations.** `laft apply`, `rollback`, `switch`, `gens`, `boot` and `gc` build, list, compare, switch and collect generations. Their integration tests run against temporary roots with a signed `file://` repository and real package files. The [virtual machine test](./try-it) applies a configuration with a new service and a new host name on the running machine, and checks without a reboot that the service runs and the host name changed.

<span class="works">works</span> **Boot counting and fallback.** New boot defaults are on trial, failed attempts are counted, and `laft-boot` falls back to the last good generation. The decisions are unit tested, including a power loss after every step of the fallback.

<span class="works">works</span> **Crash safety.** Every transaction is journaled, and the tests interrupt transactions and live switches after every recorded step and check that recovery leaves a consistent system.

<span class="works">works</span> **The configuration.** The whole schema described in the [reference](../reference/system) is checked, validated and turned into a generation: packages, alternatives, users and groups, files in `/etc`, kernel modules and sysctl settings, filesystems, swap, LUKS volumes, the boot loader and initramfs, services and gettys.

<span class="works">works</span> **Packages and repositories.** Package files, signed indexes with generation numbers that never decrease, two-level keys, mirrors with failover, and dependency resolution with explanations for every choice and every failure.

<span class="works">works</span> **The bootstrap.** The 89 recipes of the base system bootstrap from a cross toolchain through all four stages, with their test suites in stage 3. The self-built system installs and boots in the virtual machine test. In the latest run on the development machine, `check-repro` found every package of stage 3 reproducible byte for byte.

## Implemented, but early

These exist and have unit or namespace tests, but have not yet been tested on a booted machine. The plan for those tests is in `tests/vm/README`.

- **Declared views and `laft-enter`.** Additional `/usr` trees and the setuid helper that enters them.
- **Rescue.** `laft rescue` and `laft chroot` from a rescue system.
- **LUKS, xfs, btrfs and f2fs roots** in `laft-boot`, and the emergency shell paths.
- **The service catalog.** `std/services` declares `sshd`, `dbus`, `seatd`, `elogind`, `dhcpcd`, `iwd`, `chronyd` and `nftables`, but most of the packages behind them have no recipes yet.

## Planned

<span class="planned">planned</span> **Package options and profiles.** Build-time options per package, with a set of prebuilt profiles, so that a machine can choose features without building everything itself.

<span class="planned">planned</span> **Static musl variants.** The `static` variant exists in the package format, the resolver and the builder, and the test system already runs static busybox and runit built against musl. The recipes of the base system build only the dynamic variant against glibc so far.

<span class="planned">planned</span> **Git sources.** The recipe schema has `git`, `commit` and `submodules` for sources fetched from a git repository at one commit, but `laft-build` refuses such a source until it can make a reproducible tarball of the checkout. Recipes use release tarballs meanwhile.

<span class="planned">planned</span> **User views.** Views that users declare and manage for themselves.

<span class="planned">planned</span> **Hosted, signed repositories.** The `core` repository in the base part points at a placeholder URL until an official repository is published. Until then, packages come from repositories you build and serve yourself.

<span class="planned">planned</span> **Real hardware.** Installs on physical machines, with the firmware, drivers and kernel configuration they need.

<span class="planned">planned</span> **A desktop.** `std/desktop` already describes a Wayland desktop for one user, with a choice of compositor, but its packages do not exist yet.

## Following along

The source is at [github.com/QaidVoid/laft](https://github.com/QaidVoid/laft), and the configuration language at [github.com/QaidVoid/stav](https://github.com/QaidVoid/stav). The fastest way to see where things stand is to run [the virtual machine test](./try-it) yourself.
