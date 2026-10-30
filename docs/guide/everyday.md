# Everyday use

<p class="lead">The <code>laft</code> command does everything on a running system: it plans and applies the configuration, moves between generations, cleans up, and answers questions about packages, services and <code>/etc</code>. The outputs here come from the test machine of <a href="./try-it">Try it in QEMU</a>.</p>

Commands that only read never take the lock and never need root. Commands that change the system take an exclusive lock and need root, except with `--root <dir>`, which works on a system mounted at `<dir>` and activates nothing on the running machine. Results go to standard output, and progress, warnings and errors to standard error. Every command accepts `--json`, and the [command reference](../reference/laft) lists every option.

## Applying the configuration

Edit `/etc/laft/system.stav`, then look at what would change:

```console
# laft plan
evaluating  /etc/laft/system.stav  2 files  72 ms
resolving   system
generation 2 (from 1)
  system            = unchanged
  services
    + hello

  download 0 B - store +0 B - 0 packages
```

`laft apply` shows the same plan, asks before it continues, builds the new generation and activates it:

```console
# laft apply -m "add the hello service"
...
proceed? [Y/n] y
assembling  generation 2  621 ms
boot        entry 2 installed
activating  ok mount ok switch ok views ok etc ok accounts ok services ok unmount
done        generation 2 is active and default
```

The steps of the activation line are the live switch: mount the new generation, switch `/run/laft/system` to it, update the other views, `/etc` and the accounts, reconcile the services and unmount the old generation. When the configuration produces the same system, `laft apply` prints `system is up to date` and changes nothing.

Useful options of `apply`:

| Option | Effect |
|---|---|
| `-m <text>` | Store a message with the generation, shown by `laft gens list`. |
| `--diff` | Print unified diffs of the `/etc` files that change, before activating. |
| `--boot` | Build the generation and make it the default for the next boot, without activating it now. |
| `--build-only` | Build the generation and do nothing else. |
| `--dry-run` | Only show the plan, like `laft plan`. |
| `--adopt <path>` | Move an unmanaged file in `/etc` aside instead of failing when the new generation manages that path. |
| `--offline` | Never use the network. Indexes and packages come only from `file://` repositories and from what was downloaded before. |
| `-y` | Do not ask. |

`laft upgrade` applies with the newest versions of the named packages, or of every package when none is named. Plain `laft apply` keeps installed versions as long as they still satisfy everything.

The new generation takes effect on the running system at once. Services that it adds start, services that it removes stop, and changed services restart or run again as their `on_change` says. The early steps that read a changed file in `/etc`, such as `hostname`, run again too. Check the result without a reboot:

```console
# laft apply -m "add the hello service"
...
activating  ok mount ok switch ok views ok etc ok accounts ok services ok unmount
done        generation 2 is active and default
# laft svc status hello
SERVICE  STATE  READY  PID  UPTIME
hello    up     yes    174  0 ms
# uname -n
laft-demo
```

A change that only a reboot can apply, such as one to an early longrun, is listed as `services    <name> changes on the next boot`. `laft apply --boot` builds the generation and makes it the default for the next boot without touching the running system.

## Generations

```console
# laft gens list
GEN  FLAGS  CREATED           PKGS  KERNEL    MESSAGE
2    * d    2026-10-30 17:16  5     7.2.13_1  add the hello service
1    b      2026-10-30 17:13  5     7.2.13_1
```

`*` marks the active generation, `b` the booted one and `d` the default for the next boot. Here generation 2 is active and default, while the machine is still running the generation it booted.

```console
# laft gens show 1
generation   1
parent       -
created      2026-10-30 17:13
laft         0.1.0
init         runit
kernel       7.2.13_1
index        core current generation 1793380404
packages     5
```

`laft gens show <N> --packages` lists every package. `laft gens diff <A> [<B>]` compares two generations: packages, `/etc` paths, services and every configuration value that changed. Add `--etc` for unified diffs of the changed files.

```console
# laft gens diff 1 2
generation 1 -> 2
  etc
    ~ hostname
    ~ hosts
  services
    + hello
    ~ hostname
  config hostname: "laft-vm" -> "laft-demo"
  config services.hello.after: - -> []
  config services.hello.cwd: - -> "/"
  config services.hello.description: - -> "Say hello once a minute"
  ...
```

`laft gens rm <N>...` deletes generations and collects garbage afterwards, unless `--no-gc` is given. The active, booted and default generations are never deleted.

## Rolling back

`laft rollback` switches to the parent of the active generation. `laft switch <N>` switches to any generation that still exists. Neither asks for confirmation, and neither builds anything, because the generation is already complete.

```console
# laft rollback
activating  ok mount ok switch ok views ok etc ok accounts ok services ok unmount
done        generation 1 is active and default
```

Both take `--boot` to change only the default for the next boot. The boot menu lists every generation, so an older one can also be picked by hand at boot.

## Boot entries

```console
# laft boot list
#  GEN  KERNEL    DEFAULT  FILES
1  2    7.2.13_1  yes      ok
2  1    7.2.13_1           ok
# laft boot show
generation  1
status      good
kernel      7.2.13_1 (y6jikgwolsdgntx6mkx5ard3ab3ymkdz)
initramfs   rkj7a4qcwuv452twrwlc6zc5vxmpoedo
cmdline     laft.gen=1 console=ttyS0 quiet laft.debug
modules     virtio_ring virtio virtio_pci_legacy_dev virtio_pci_modern_dev virtio_pci virtio_blk crc16 mbcache jbd2 ext4 fat vfat nls_cp437 nls_iso8859_1
```

`laft boot log` shows boot events, such as a fallback after failed attempts. `laft boot set-default <N>` changes the default without activating anything, and `laft boot install` renders the EFI system partition again from the generations. `laft boot mark-good` records that the booted generation started completely, which the `boot-complete` service does on every boot.

## Collecting garbage

After every successful apply, Laft collects garbage on its own, unless `gc { auto = false }` or `--no-gc`. To see what a collection would do, and why:

```console
# laft gc --dry-run --explain
kept     gen 2          active system, default, newest 10, younger than 30 days
kept     gen 1          booted, newest 10, younger than 30 days
kept     runit 2.3.1-r1+static gen 2, gen 1
kept     laft 0.1.0-r1  gen 2, gen 1
kept     busybox 1.37.0-r1+static gen 2, gen 1
kept     linux 7.2.13-r1 gen 2, gen 1
kept     limine 11.2.1-r1 gen 2, gen 1
would remove 0 generations, 0 view versions, 0 store entries, 0 boot files - freed 0 B
```

The configuration's `gc` block sets how many generations to keep and for how many days. `--keep-generations` and `--keep-days` override it for one run.

## Services

```console
# laft svc status hello
SERVICE  STATE  READY  PID  UPTIME
hello    up     yes    174  0 ms
# laft svc show hello
service     hello
kind        longrun
exec        /usr/bin/sh -c while true; do echo hello; sleep 60; done
user        root
after       early
ready       immediately
restart     always
log         /var/log/hello
generation  /run/laft/system/services/spec
```

Without names, `laft svc status` shows every enabled service. `laft svc start`, `stop` and `restart` change services until the next apply or boot. The configuration stays the source of truth, so a service that should stay off gets `enable = false`. `laft svc log <name>` prints the last 50 lines of a service's log from `/var/log/<name>/current`, `-n` changes the count and `-f` follows it.

A service with `log = console` has no log file. Its output goes to `/dev/console`, which is the last `console=` device of the kernel command line, the serial port on the test machine. `laft svc log` says so instead of failing on the missing file, and it says the same of a service with `log = discard`, whose output is thrown away:

```console
# laft svc log hello
2026-10-30_18:01:42.08238 hello
# laft svc log boot-complete
error: service boot-complete logs to the console, so it has no log file: its output goes to /dev/console, which is the last console= device of the kernel command line
```

## `/etc`

```console
# laft etc status
/etc matches the active generation
```

`laft etc status` lists managed paths that differ from the active generation, for example a link someone replaced with a file. `laft etc diff [<path>...]` shows how they differ, and `laft etc reset <path>...` restores them.

## Packages

```console
# laft info runit:static
name         runit
version      2.3.1-r1
variants     static
repo         core
summary      A UNIX init scheme with service supervision
homepage     https://smarden.org/runit/
license      BSD-3-Clause
download     236.8 KiB
installed    868.3 KiB
store id     7ah4dyc33f53wzrceczzpaspjwp5nhjx
views        system (2.3.1-r1)
# laft files runit:static
/usr/
/usr/bin/
/usr/bin/chpst
/usr/bin/runit
...
# laft owns /usr/bin/sv
/usr/bin/sv is owned by runit 2.3.1-r1 (static)
# laft why runit
runit 2.3.1-r1 is in view "system" because:
  requested at /etc/laft/system.stav:9:31 (runit:static)
```

`laft search <text>` searches names and summaries in the synced indexes. `laft owns` also accepts a bare command name and looks for it in `/usr/bin`. `laft why` explains every view with `--all`, and follows dependencies when a package was not requested directly. It names configuration files as the installed system sees them, also for a generation that `laft install` or `--root` built below a mount point.

## Checking the system

```console
# laft verify
ok 5 packages
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
```

`laft verify` checks the store entries of the active generation, or of the packages it names. `--full` hashes every file, `--links` checks that the files of generations and views are the store's own files, and `--all` covers every entry and generation. `laft doctor` runs health checks of the layout, mounts, journal, free space, indexes and boot files. Both exit with status 9 when they find a problem.

## Repositories

`laft repo sync` fetches the indexes of every enabled repository, or of the named ones. Commands sync on their own when an index is stale, and `--refresh` forces it. `laft repo list` shows the configured repositories. `laft repo reset <name>` forgets the index and keys accepted from a repository, so the next sync trusts it afresh from its configured key.

## Installing and rescuing

`laft install` installs onto filesystems mounted at `--root`. This is the sequence the test's installer runs, with the root filesystem at `/mnt` and the EFI system partition at `/mnt/boot`:

```sh
mkdir -p /mnt/etc/laft
cp system.stav /mnt/etc/laft/system.stav
laft --root /mnt hardware --write
laft --root /mnt --yes install
```

`laft hardware --write` describes the mounted filesystems, swap, LUKS volumes and CPU in `etc/laft/hardware.stav`, and never overwrites that file once it exists. `laft install --generate-config` writes a starting `system.stav` together with it. `laft install` refuses a target that holds anything but `boot/`, `lost+found/` and `etc/laft/`, creates the layout and builds generation 1:

```text
done        generation 1 is the default for the next boot
next steps:
  laft rescue --root /mnt -- passwd root
  umount -R /mnt
  reboot
```

`laft rescue --root <dir> [-- <command>]` and `laft chroot <dir>` enter a mounted Laft system from a rescue environment, in a private mount namespace with the default generation, or the one `--gen` names, mounted as its `/usr`.

::: info Installing on real hardware
`laft install` is exercised by the virtual machine test. Installs on real machines are [planned](./status) and not supported yet.
:::

## Exit statuses

Scripts can tell failures apart by the exit status of `laft`:

| Status | Meaning |
|---|---|
| 0 | Success, including nothing to do. |
| 1 | General failure. |
| 2 | Usage error, or a confirmation that nobody can give. |
| 3 | Activated, but a service did not become ready. |
| 4 | Resolution error. |
| 5 | Network, index trust or package integrity error. |
| 6 | Path, alternatives or `/etc` conflict. |
| 7 | The lock is busy and `--no-wait` was given. |
| 8 | Activated, but an activation step failed. |
| 9 | `verify` or `doctor` found problems. |
| 10 | Declined at a confirmation. |
| 11 | Configuration error. |
| 12 | `plan --exit-code` found changes. |
