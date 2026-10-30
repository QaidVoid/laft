# What Laft is

<p class="lead">Laft is a Linux distribution and the Rust toolchain that builds and runs it. A machine is described by one declarative file. Applying that file builds a new, immutable generation of the whole system from a content addressed package store, and the machine switches to it in one step.</p>

::: warning Pre-release
Laft is not ready for real machines or daily use. It installs and boots in a virtual machine, and its test suites exercise most of what this guide describes. The [status page](./status) lists what works and what is still planned.
:::

## The idea in one paragraph

Most distributions change a running system in place: a package manager unpacks files over the old ones and runs scripts as it goes. Laft never edits a live system tree. Packages are unpacked once into `/laft/store`, each in its own immutable directory named by a hash of its contents. A **view** is a complete `/usr` assembled from store entries with hardlinks. A **generation** is a numbered snapshot of the whole system: the system view, the rendered `/etc`, the services, the accounts and the boot files. `/usr` is a symlink through `/run/laft/system`, so moving from one generation to the next is a single `rename(2)`, and rolling back is the same operation in the other direction.

## What it is made of

| Piece | What it does |
|---|---|
| [Stav](https://github.com/QaidVoid/stav) | The typed, total configuration language that system files and recipes are written in. |
| `laft` | Plans, applies, switches and inspects a system. It also installs one onto a mounted disk. |
| `laft-boot` | The init of the initramfs. It picks a generation, counts the boot attempt, mounts the generation and starts runit. |
| `laft-enter` | A small setuid helper that runs a command inside a view other than the system's. |
| `laft-build` | Turns recipes into packages in sandboxes without network, and runs the bootstrap. |
| `laft-repo` | Indexes, signs, checks and publishes package repositories. |

Every one of these is written in Rust, linked statically against musl and uses mimalloc. None of them needs anything in `/usr` to run, which is what lets them repair a system whose `/usr` is broken or empty.

The packages themselves are ordinary Linux software built from source: glibc, GCC, coreutils, util-linux, udev, runit, Limine, the kernel and the rest of a base system. The init system is runit, the boot loader is Limine, `/bin/sh` is dash, and every machine boots through UEFI.

## What makes it different

**One file describes the machine.** Packages, users and groups, files in `/etc`, kernel modules and parameters, filesystems, swap, LUKS volumes, the boot loader, services and the terminals that get a login prompt all live in `/etc/laft/system.stav`. Stav checks the whole configuration before anything happens, so a typo in a service name or a reference to a user that does not exist stops `laft apply` with a diagnostic instead of a half-configured system. See [Configuring a system](./configuration).

**Many versions can coexist.** The store is addressed by content. Two packages with the same name but different versions, or the same version built two ways, are two different directories. A view picks exactly one of each name, but different views and different generations can pick differently, and they share every file they have in common. See [Core concepts](./concepts).

**Changes are atomic and reversible.** A new generation is built completely, next to the running one, before anything switches. The switch is one rename. If the machine loses power at any moment, it boots either the old generation or the new one. Old generations stay in the boot menu, and `laft rollback` returns to the previous one. See [Everyday use](./everyday).

**No package runs code on your machine.** Packages carry files and metadata, never install scripts. Caches that depend on what a view contains, such as `modules.dep` or the font cache, are produced by triggers from a fixed table that Laft itself implements.

**Builds are hermetic and checked.** Every source is pinned by URL and SHA-256. Every build runs in a sandbox whose `/usr` holds only what the recipe declares, with no network. The base system is bootstrapped from a cross toolchain, rebuilt twice inside the sandbox, and checked to rebuild byte for byte. See [Building packages](./building).

## Where to go next

- [Try it in QEMU](./try-it) to boot a Laft system on your own machine in a few minutes.
- [Core concepts](./concepts) for the store, views, generations, `/etc` and services in more depth.
- [Writing recipes](./recipes) if you want to package software for Laft.
