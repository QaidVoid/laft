# Core concepts

<p class="lead">Five ideas explain how a Laft system is put together: the store, packages, views, generations and the way <code>/etc</code> and services follow the active generation. The listings on this page come from the machine that <a href="./try-it">the virtual machine test</a> installs.</p>

Everything Laft manages lives below `/laft`:

```console
# ls /laft
cache  gens   state  store  tmp    views
```

| Directory | Holds |
|---|---|
| `store` | One immutable directory per package. |
| `views` | Versions of the views besides the system view. |
| `gens` | Numbered generations and the `default` link to the one that boots. |
| `state` | Boot counting, accepted repository indexes and pins. |
| `cache` | Downloaded indexes and package files. |
| `tmp` | Staging space for transactions, on the same filesystem. |

`/laft` is mounted read-only, `nosuid` and `nodev`. Laft writes to it only from a private mount namespace, so the writable mount never appears to the rest of the system.

## The store

A package is unpacked exactly once, into `/laft/store`. Its directory is named by its **store id**, the first 160 bits of the BLAKE3 hash of its canonical payload manifest, written as 32 base32 characters. The name and version after the id are only there for people reading a listing. Laft reads a package's identity from its manifest and never parses it back from the path.

```console
# ls /laft/store
7ah4dyc33f53wzrceczzpaspjwp5nhjx-runit-2.3.1+static
igyza4pekasw7tig4ana2lqhgotoy3ek-laft-0.1.0
k7pjia6tynb7vepkdzemiobizilxp7tl-busybox-1.37.0+static
y6jikgwolsdgntx6mkx5ard3ab3ymkdz-linux-7.2.13
yibf6gdfl34n22efd5uiz6obecmbacko-limine-11.2.1
```

An entry is published with one rename and never written to again. Because the id is derived from the content, two packages that differ in any file have different ids, and the same package reached twice, from a mirror or from a rebuild, has the same id. Many versions of one package can sit side by side in the store without touching each other.

Store files never carry setuid, setgid or file capabilities. Programs that need privileges get them from the [privileged copies](#privileges) of the active generation instead.

## Packages

A package file (`.lpkg`) is a pax tar archive compressed with zstd. Its first member is the metadata, `.laft/meta.json`, its second the payload manifest, `.laft/manifest.json`, and the payload follows. Both documents are canonical JSON as defined by RFC 8785, so the same package always hashes the same way.

Packages are named by a **reference** of the form `name[@prefix][:variant]`:

| Reference | Selects |
|---|---|
| `ripgrep` | the newest `ripgrep`, dynamic variant |
| `ripgrep:static` | the newest `ripgrep`, static variant |
| `llvm@19` | the newest `llvm` whose version starts with the token `19` |
| `llvm@19:static` | both constraints |

There are two **variants**. `dynamic` is the default and links against the system's shared libraries. `static` packages ship statically linked programs, which the virtual machine test uses for `busybox:static` and `runit:static`.

Dependencies are mostly detected rather than declared. When `laft-build` packages a build, it reads every ELF file of the payload and turns each `DT_NEEDED` entry into a dependency such as `so:libc.so.6`, and each `DT_SONAME` into something the package provides. It also records an ABI digest of every shared library, so it can tell whether a rebuilt library still satisfies what links against it. A recipe declares only what detection cannot see, such as the interpreter of a script.

A build usually produces several packages. Besides the main one, `-dev` holds headers and link-time files, `-doc` the documentation and `-dbg` the split debug info.

## Views

A **view** is a complete `/usr` tree assembled from store entries with hardlinks. The resolver picks exactly one version of each package name for a view, from the requests in the configuration and everything those packages depend on, and assembly links every file of every chosen entry into place. Nothing is copied:

```console
# ls -i /usr/bin/runsv /laft/store/*-runit-*/usr/bin/runsv
    509 /laft/store/7ah4dyc33f53wzrceczzpaspjwp5nhjx-runit-2.3.1+static/usr/bin/runsv
    509 /usr/bin/runsv
```

Two packages never own the same path in one view. Packages that offer the same paths on purpose join an **alternatives** group in their recipes, and the `alternatives` setting of the configuration picks the member that provides them.

While a view is assembled, Laft runs the **triggers** its packages activate: `depmod` for kernel modules, `fontconfig`, `glib-schemas`, `gdk-pixbuf`, `gio-modules`, `gtk3-immodules`, `icons`, `info`, `mime`, `desktop` and `udev-hwdb`. They come from a fixed table in Laft and run in a sandbox as `nobody`, with only their own output directories writable. A package never runs code of its own on the machine it is installed on.

The **system view** is the one every process sees as `/usr`. Further views are declared with `view` blocks in the configuration and live in `/laft/views/<name>/<version>`. Each one starts from the system's package requests, or from nothing, and can pin other versions. The setuid helper `laft-enter` runs a command with such a view mounted over `/usr` in a private mount namespace, and `laft enter <view> -- <command>` calls it.

::: info Tested in unit and namespace tests
Declared views and `laft-enter` are implemented, with unit tests and tests inside user namespaces. Their tests on a booted machine are still to be written, so treat them as early.
:::

## Generations

A **generation** is a numbered, immutable state of the whole system:

```console
# ls /laft/gens /laft/gens/1
/laft/gens:
1        default

/laft/gens/1:
accounts.json    etc              services
boot             gen.json         triggers.json
config.json      privileged.json  usr
```

`usr` is the system view, `etc` the rendered static `/etc`, `services` the rendered runit service directories, `boot` the kernel and initramfs with their boot record, and `gen.json` the manifest that lists every package with its store id and the reason it is there. A generation without a valid `gen.json` is incomplete, and Laft never boots or activates it.

The top-level directories of the root point into the active generation:

```console
# ls -l / | grep -E ' (bin|lib|usr)'
lrwxrwxrwx    1 root     root             7 Oct 30 17:13 bin -> usr/bin
lrwxrwxrwx    1 root     root             7 Oct 30 17:13 lib -> usr/lib
lrwxrwxrwx    1 root     root             7 Oct 30 17:13 lib64 -> usr/lib
lrwxrwxrwx    1 root     root             7 Oct 30 17:13 sbin -> usr/bin
lrwxrwxrwx    1 root     root            20 Oct 30 17:13 usr -> /run/laft/system/usr
```

`/run/laft/system` is a symlink to `mounts/<N>`, and `/run/laft/mounts/<N>` is generation N, mounted read-only. Switching generations mounts the new one next to the old one and replaces the `/run/laft/system` link with a single `rename(2)`. Every path below `/usr` changes at that instant, and there is no moment at which `/usr` is half old and half new.

Every change runs as a **transaction** under one exclusive lock, and each step is written to a journal before it happens. If a command is killed or the power fails, the next `laft` command finishes or undoes the transaction from the journal, and the machine boots either the old default or the new one.

`laft apply` writes a new generation, `laft rollback` switches to the parent of the active one, and `laft switch <N>` to any generation that still exists. Garbage collection keeps the newest ten generations and every generation younger than thirty days by default, together with the active, booted and default ones. It deletes the rest, and then every store entry that no kept generation or view uses.

## `/etc`

`/etc` is a real directory, not a view. Laft renders the files a generation manages into the generation's own `etc`, which `/etc/static` follows, and links each managed path to it:

```console
# ls -l /etc
lrwxrwxrwx    1 root     root            17 Oct 30 17:13 fstab -> /etc/static/fstab
-rw-r--r--    1 root     root           395 Oct 30 17:15 group
lrwxrwxrwx    1 root     root            20 Oct 30 17:13 hostname -> /etc/static/hostname
lrwxrwxrwx    1 root     root            17 Oct 30 17:13 hosts -> /etc/static/hosts
-rw-r--r--    1 root     root           559 Oct 30 17:15 passwd
lrwxrwxrwx    1 root     root            20 Oct 30 17:13 static -> /run/laft/system/etc
...
```

Because the links go through `/etc/static`, switching generations switches the content of every managed file at the same instant as `/usr`. Files declared with `copy = true` are written as real files instead.

Laft never overwrites a file it did not create. If a new generation wants to manage a path that already holds an unmanaged file, `laft apply` stops with exit status 6 before anything changes, and `--adopt <path>` moves the old file aside. `laft etc status` lists managed paths that someone changed, and `laft etc reset` restores them.

The account databases `passwd`, `group`, `shadow` and `gshadow` are merged rather than linked. Laft writes the declared accounts into them and, unless `users { mutable = false }`, keeps accounts that were created another way. The static system accounts, such as `root`, `wheel` and `messagebus`, have the same ids on every Laft system.

## Services

Services are declared once, in the configuration, in a model that does not depend on the init system. Laft validates them, resolves each one into a complete spec and renders the specs into service directories for runit.

A service is a `longrun`, which runit supervises, or a `oneshot`, which runs an `up` command on start and an optional `down` command on stop. It can name the services it starts `after`, a readiness check (`immediate`, a notification descriptor, or a command polled until it succeeds), the user and groups it runs as, its environment, resource limits, state and runtime directories, a restart policy and where its output goes. Laft's own launcher does what runit cannot express: it waits for dependencies to be ready, checks readiness, creates directories, sets limits and drops privileges.

Every system includes a base part with the **early boot graph**: oneshots that reconcile `/etc` with the booted generation (`laft-activate`), mount the kernel filesystems (`kernel-fs`), load modules (`kmod`), start udev, set the clock and host name, check and mount filesystems (`mount-fs`), enable swap, carry the random seed across boots and apply sysctl settings. They end in the oneshot `early`, which every other service implicitly waits for. Several of these steps are built into `laft` itself and run as `laft svc builtin <step> up`.

When the system switches generations, Laft compares the old and new service sets. It stops services that were removed, starts new ones and, for services whose spec changed, does what `on_change` says: `restart`, `rerun`, `ignore` or `reboot`. The early steps `hostname`, `console`, `kmod` and `sysctl` read files in `/etc`, so they also count as changed when those files change, and they run again. A changed `/etc/fstab` takes effect at the next boot, because running `mount-fs` again would first unmount filesystems that may be in use.

## Boot

`laft-boot` is the init of the initramfs. It mounts the kernel filesystems, loads the modules the initramfs carries, waits for the boot devices, unlocks LUKS volumes, checks and mounts the root, chooses the generation, mounts it, makes `/laft` read-only and executes the generation's init. A failure before the switch drops to an emergency shell and retries the failed step.

Each new boot default is on trial. Every boot of it uses one of `boot.tries` attempts, three by default. When a boot completes, the `boot-complete` service runs `laft boot mark-good` and the trial ends. If the attempts run out, `laft-boot` marks the generation bad and falls back to the last generation that booted completely. The state lives in `/laft/state/boot.json`:

```console
# cat /laft/state/boot.json
{"bad":[],"events":[],"good":[1],"last_good":1,"trial":null,"version":1}
```

## Repositories and trust

A repository is a directory of static files: package files in `pool/`, their build records in `buildinfo/`, and one signed index per channel and architecture. Indexes are canonical JSON compressed with zstd and signed with minisign. The signature's trusted comment names the repository, channel, architecture and a generation number, and a client refuses an index whose generation is lower than one it already accepted, as well as expired indexes and revoked keys. Packages are downloaded by BLAKE3 hash, so a mirror can serve them but cannot change them.

A configuration names repositories with `repo` blocks, each with a URL and a root public key. The official repository is [not published yet](./status), so the virtual machine test publishes its own repository with `laft-repo` and serves it from the installer.

## Privileges

Store files never carry setuid, setgid or capability bits. The configuration lists privileged files under `security { privileged "<path>" { ... } }`, and each generation keeps its own copies of those files with the requested bits, inside the generation directory. That directory is mounted without `nosuid` only for the active system generation. Old generations and the files of views never carry or honor privileges.
