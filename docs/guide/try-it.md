# Try it in QEMU

<p class="lead">The repository carries an end-to-end test that builds a few packages, publishes them as a signed repository, installs a Laft system onto a disk image and boots it under UEFI. Run it once with <code>--keep</code>, and the disk image stays behind for you to boot and explore.</p>

## What you need

- An x86-64 Linux host. Nothing runs as root.
- [rustup](https://rustup.rs). The repository pins its toolchain in `rust-toolchain.toml`, including the `x86_64-unknown-linux-musl` target, and rustup installs it on first use.
- `qemu-system-x86_64` with its UEFI firmware at `/usr/share/qemu/edk2-x86_64-code.fd` and `/usr/share/qemu/edk2-i386-vars.fd`. The test looks for the firmware at exactly those paths.
- The musl cross compiler `x86_64-linux-musl-gcc` and its tools, which the static Laft binaries and the static packages are built with.
- `gcc`, `make` and `zstd`, which the package builds use from the host, and `sfdisk`, `mkfs.vfat` and `mkfs.ext4`, which make the disk image.
- 1.5 GiB of available memory for each machine, and 10 GiB of free disk space in `~/.cache`, which `laft-build` checks before it builds.
- `/dev/kvm`, ideally. Without it the machines are emulated, and everything takes about ten times longer.
- A network connection for the first run, which downloads the pinned sources.

Laft depends on [Stav](https://github.com/QaidVoid/stav) through a checkout next to it, so clone both into the same directory:

```sh
cd ~
git clone https://github.com/QaidVoid/stav
git clone https://github.com/QaidVoid/laft
cd laft
```

## Run the test

```sh
tests/vm/run --keep
```

`tests/vm/run` builds the static Laft binaries with four jobs, then runs `laft-vmtest`, the harness in `tests/vm/src`. Every step prints a line, and the run ends with:

```text
laft-vmtest: ok: generation 1 is still good
laft-vmtest: ok: nothing is bad
laft-vmtest: ok: runit stage 3
laft-vmtest: ok: the power off
laft-vmtest: ok: the machine powered off
laft-vmtest: passed
```

The first run downloads and builds its packages. Later runs find them up to date and finish in well under a minute with KVM.

### What it does

1. **Build.** `laft-build` builds busybox and runit as static packages from the recipes in `tests/vm/recipes`, and packages Limine and a prebuilt kernel. Every source is pinned by URL and SHA-256 and verified before use. These builds use the host's compiler in bootstrap mode, without the sandbox, which is fine for a test and never used for published packages.
2. **Publish.** The Laft binaries become the `laft` package, and `laft-repo` signs all five packages into a `file://` repository with a fresh key.
3. **Install.** A GPT disk image gets an empty FAT32 EFI system partition and an empty ext4 root. An installer machine boots the kernel directly, mounts both filesystems, runs `laft hardware --write` to describe them, and runs `laft install` with the configuration in `tests/vm/system.stav`.
4. **Boot.** The disk boots through the UEFI firmware, Limine and `laft-boot` into runit. The harness watches the serial console for every stage, logs in as root, and checks that every early service succeeded, that `/run/laft/system` points at generation 1, and that `/laft` is mounted read-only, `nosuid` and `nodev`.
5. **Boot again.** A second boot checks that the first one was recorded as good in `/laft/state/boot.json`.

Everything lives in `~/.cache/laft-vm`. The disk image, the firmware variables and the console log of each machine are in `~/.cache/laft-vm/work`:

| File | Contents |
|---|---|
| `disk.img` | The installed system, kept by `--keep` |
| `vars.fd` | The UEFI variables of the machine, kept by `--keep` |
| `install.log` | The console of the installer, with the output of `laft install` |
| `boot-1.log`, `boot-2.log` | The consoles of the two boots |

## Boot it yourself

Start the kept image with the same machine the test uses, with its serial console in your terminal:

```sh
cd ~/.cache/laft-vm/work
qemu-system-x86_64 -machine q35,hpet=off -smp 2 -m 1024 \
  -accel kvm -cpu host \
  -nographic -vga none -nic none \
  -drive file=disk.img,format=raw,if=virtio \
  -drive if=pflash,format=raw,unit=0,readonly=on,file=/usr/share/qemu/edk2-x86_64-code.fd \
  -drive if=pflash,format=raw,unit=1,file=vars.fd
```

Without KVM, replace `-accel kvm -cpu host` with `-accel tcg -cpu max`. The machine has no HPET, because with UEFI firmware it would leave the CMOS clock without interrupts, and `hwclock` waits for one at boot.

Limine shows its menu for a second, `laft-boot` picks the generation, and runit starts the services:

```text
laft-boot: switching to generation 1 on /dev/vda2
- runit: $Id: c66c1e3b98d1b630bd51b1d56aaed5c2994e8c9d $: booting.
- runit: enter stage: /etc/runit/1
activated   generation 1 at boot
- runit: leave stage: /etc/runit/1
- runit: enter stage: /etc/runit/2
generation 1 booted completely
laft-vmtest: marker service is running
laft-vm login:
```

Log in as `root` with the password `laft`. The shell is busybox `sh`.

To leave, power the machine off with `runit-init 0`, or press <kbd>Ctrl</kbd>+<kbd>a</kbd> then <kbd>x</kbd> to stop QEMU at once. Booting the image changes it, as it would change a real disk. Run the test again for a fresh one.

## Look around

The system has five packages and nothing else. Every command below was run on it.

```console
# laft gens list
GEN  FLAGS  CREATED           PKGS  KERNEL    MESSAGE
1    * b d  2026-10-30 17:13  5     7.2.13_1
```

The flags say that generation 1 is active (`*`), booted (`b`) and the default (`d`).

```console
# laft svc status
SERVICE        STATE  READY  PID  UPTIME
boot-complete  done   yes    112  3.0s
clock          done   yes    -    -
console        done   yes    -    -
early          done   yes    -    -
getty-ttys0    up     yes    111  3.0s
hostname       done   yes    -    -
kernel-fs      done   yes    -    -
kmod           done   yes    -    -
laft-activate  done   yes    -    -
mount-fs       done   yes    -    -
random-seed    done   yes    -    -
swap           done   yes    -    -
sysctl         done   yes    -    -
udev-trigger   done   yes    -    -
vmtest-marker  up     yes    109  3.0s
```

Most of these are the early boot graph that every system includes. `vmtest-marker` and the getty come from the configuration.

```console
# laft why runit
runit 2.3.1-r1 is in view "system" because:
  requested at /mnt/etc/laft/system.stav:9:31 (runit:static)
# laft owns /usr/bin/sv
/usr/bin/sv is owned by runit 2.3.1-r1 (static)
# laft doctor
ok layout
ok etc static
ok run state
ok laft mount
ok generation mounts
ok journal
ok tmp
ok space
ok indexes
ok boot
ok static binaries
ok clock
# laft verify
ok 5 packages
```

The path in the `why` output is where the installer read the configuration, with the target mounted at `/mnt`. The [core concepts](./concepts) page walks through `/laft`, the store and `/etc` on this same machine.

## Change something and roll back

The configuration is in `/etc/laft/system.stav`, and busybox `vi` can edit it. Add a service and change the host name:

```stav
  hostname = "laft-demo"

  service hello {
    description = "Say hello once a minute"
    exec        = [
      "/usr/bin/sh",
      "-c",
      "while true; do echo hello; sleep 60; done",
    ]
  }
```

The test system's repository was served by the installer and is not reachable from the installed system, so pass `--offline`. Laft then works from the index it already accepted and the packages already in the store, which is all this change needs.

```console
# laft plan --offline
evaluating  /etc/laft/system.stav  2 files  72 ms
resolving   system
generation 2 (from 1)
  system            = unchanged
  services
    + hello

  download 0 B - store +0 B - 0 packages
# laft apply --offline --diff -m "add the hello service"
evaluating  /etc/laft/system.stav  2 files  25 ms
resolving   system
generation 2 (from 1)
  system            = unchanged
  services
    + hello

  download 0 B - store +0 B - 0 packages

proceed? [Y/n] y
warning: privileged "/usr/bin/doas" is inactive: the generation has no regular file at that path
...
assembling  generation 2  621 ms
--- /etc/hostname
+++ /etc/hostname
@@ -1,1 +1,1 @@
-laft-vm
+laft-demo
...
activating  ok mount ok switch ok views ok etc ok accounts ok services ok unmount
done        generation 2 is active and default
```

The warnings come from the base part, which declares privileged programs such as `doas` and `passwd` that this minimal system does not contain. Generation 2 reuses every store entry of generation 1. Only its `/etc`, its services and its configuration differ, which `laft gens diff 1 2` lists.

::: warning A known gap in live switching
On this test system, `laft apply` switches to generation 2 but does not start the new `hello` service. It warns that it could not add the service and exits with status 3, which means the generation is active but a service did not become ready. The service runs after a reboot. Live switching is implemented and covered by unit tests, but its tests on a booted machine are still being written. Booting into a new generation is the path that is tested end to end today.
:::

Reboot into the new generation with `runit-init 6`, and `hello` is up with the new host name. Then go back:

```console
# laft rollback
activating  ok mount ok switch ok views ok etc ok accounts ok services ok unmount
done        generation 1 is active and default
# laft gens list
GEN  FLAGS  CREATED           PKGS  KERNEL    MESSAGE
2           2026-10-30 17:16  5     7.2.13_1  add the hello service
1    * b d  2026-10-30 17:13  5     7.2.13_1
```

Generation 2 is still there, and `laft switch 2` returns to it. Both generations are in the boot menu, and `laft boot list` shows which one boots by default.

## The self-built system

The test can also install the base system that the [bootstrap](./building#the-bootstrap) builds from source, instead of the minimal one:

```sh
tests/vm/run --self-built ~/.cache/laft-bootstrap/build/out
```

That system has glibc, dash, bash, coreutils, util-linux, shadow, udev, runit, dosfstools, the kernel built from kernel.org sources and Limine built from source, with the configuration in `tests/vm/system-self-built.stav`. It boots without busybox, with udev populating `/dev`. Running it needs a finished bootstrap first, which takes hours.
