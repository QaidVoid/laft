# System configuration

<p class="lead">Every field of the system configuration, with its type and default, as the schema in <code>crates/laft-config/stav/prelude/system.stav</code> declares it. <code>laft options</code> prints the same list on a machine.</p>

Types are Stav types. `Option<T>` fields may be left out, `Set<T>` and `List<T>` are written as lists, and `Map<Str, T>` fields with a block keyword are written as labeled blocks: the `services` map is written as `service <name> { ... }` blocks. `Ref<User>` and `Ref<Group>` name a declared account. `Pkg` is a [package reference](../guide/concepts#packages) and `Path` a path literal relative to the file. `Data` is any value that the target format can hold. See the [Stav reference](https://github.com/QaidVoid/stav/blob/main/docs/reference/types.md) for the type syntax and `where` refinements.

Values from the base part and the `std` catalog are defaults that any plain assignment replaces. The defaults below are those of the schema itself.

## The `system` block

Every configuration has one `system` block, and every field below lives in it.

### `System`

Everything that describes one machine.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `hostname` | `Str where hostname` | `"laft"` | The host name: one label of `a-z`, `0-9` and `-`, at most 63 characters, not starting or ending with `-`. |
| `timezone` | `Str where matches(r"[A-Za-z0-9_+-]+(/[A-Za-z0-9_+-]+)*")` | `"UTC"` | The IANA time zone name, for example "Europe/Berlin". |
| `locale` | `Str where nonEmpty` | `"C.UTF-8"` | The value of LANG for logins and services. |
| `extra_locales` | `Set<Str>` | `[]` | More locales that must be available, for example "de_DE.UTF-8". |
| `keymap` | `Option<Str where matches(r"[A-Za-z0-9._-]+")>` |  | The console keymap for loadkeys, for example "de-latin1". |
| `console_font` | `Option<Str where matches(r"[A-Za-z0-9._-]+")>` |  | The console font for setfont, for example "ter-v22n". |
| `users` | `Users` | `Users {}` | The declared accounts. |
| `session` | `Session` | `Session {}` | Login session tracking. |
| `files` | `Map<Str, File>` | `{}` | Files in /etc. Block label: the absolute path. Written as `file` blocks. |
| `kernel` | `Kernel` | `Kernel {}` | Kernel modules to load, blacklist and configure. |
| `sysctl` | `Map<Str, Data>` | `{}` | Kernel parameters by key, for example "vm.swappiness". Values are integers, strings or booleans. |
| `repos` | `Map<Str, Repo>` | `{}` | Package repositories. Block label: the repository name. Written as `repo` blocks. |
| `packages` | `Set<Pkg>` | `[]` | Packages in the system view. |
| `alternatives` | `Map<Str, Pkg>` | `{}` | The chosen member of each alternatives group, for the system view. |
| `views` | `Map<Str, View>` | `{}` | Views besides the system view. Block label: the view name. Written as `view` blocks. |
| `gc` | `Gc` | `Gc {}` | Retention of generations and automatic collection. |
| `boot` | `Boot` | `Boot {}` | The boot loader, kernel, boot counting and initramfs. |
| `fs` | `Map<Str, Fs>` | `{}` | Mounted filesystems. Block label: the mountpoint. Written as `fs` blocks. |
| `swap` | `Map<Str, Swap>` | `{}` | Swap devices. Block label: the device. Written as `swap` blocks. |
| `luks` | `Map<Str, Luks>` | `{}` | LUKS devices unlocked in the initramfs. Block label: the device-mapper name. Written as `luks` blocks. |
| `init` | `Init` | `runit` | The init system. |
| `services` | `Map<Str, Service>` | `{}` | System services. Block label: the service name. Written as `service` blocks. |
| `getty` | `Getty` | `Getty {}` | The terminals that get a login prompt. |
| `security` | `Security` | `Security {}` | Files that keep setuid, setgid or file capabilities. |

## Accounts

### `Users`

Account settings.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `mutable` | `Bool` | `true` | true: accounts created outside Laft are kept. false: the account databases contain exactly the declared accounts. |
| `accounts` | `Map<Str, User>` | `{}` | Users. Block label: the user name. Written as `user` blocks. |
| `groups` | `Map<Str, Group>` | `{}` | Groups. Block label: the group name. Written as `group` blocks. |

### `User`

A user account. Block label: the user name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `uid` | `Option<Int where 0..=65534>` |  | Allocated when none. |
| `group` | `Option<Ref<Group>>` |  | The primary group. none: a group with the user's name, created when it is not declared. |
| `groups` | `Set<Ref<Group>>` | `[]` | Supplementary groups. |
| `description` | `Str` | `""` | The comment field of /etc/passwd. |
| `home` | `Option<Str where matches(r"/.*")>` |  | none: /home/&lt;name&gt;, or /var/empty for system accounts. |
| `create_home` | `Option<Bool>` |  | none: true for regular accounts, false for system accounts. |
| `shell` | `Option<Str where matches(r"/.*")>` |  | none: /usr/bin/bash, or /usr/bin/nologin for system accounts. |
| `system` | `Bool` | `false` | System accounts take their ids from the system range. |
| `password_hash` | `Option<Str where matches(r"!\|\*\|!?\$(y\|gy\|6\|5\|2b)\$.+")>` |  | A crypt(3) hash, or "!" to lock the account. |
| `password_file` | `Option<Str where matches(r"/.*")>` |  | The absolute path of a file holding the hash, read at activation. |
| `ssh_keys` | `List<Str>` | `[]` | OpenSSH public keys that may log in as the user. |

### `Group`

A group. Block label: the group name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `gid` | `Option<Int where 0..=65534>` |  | Allocated when none. |
| `members` | `Set<Ref<User>>` | `[]` | Members in addition to the users that list this group in `groups`. |
| `system` | `Bool` | `false` | System groups take their ids from the system range. |

**`RundirOwner`**: Who creates and removes /run/user/&lt;uid&gt;. `turnstile | elogind`

### `Session`

Login session tracking.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `rundir` | `RundirOwner` | `turnstile` | Who creates and removes /run/user/&lt;uid&gt;. |

## Files and the kernel

### `File`

A file in /etc. Block label: the absolute path. Exactly one content field is set when the file is enabled.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `text` | `Option<Str>` |  | The content, verbatim. |
| `source` | `Option<Path>` |  | A file whose bytes are the content. |
| `link` | `Option<Str where nonEmpty>` |  | A symlink with this target. |
| `json` | `Option<Data>` |  | A value rendered as JSON. |
| `toml` | `Option<Data>` |  | A map rendered as TOML. |
| `yaml` | `Option<Data>` |  | A value rendered as YAML. |
| `ini` | `Option<Map<Str, Map<Str, Data>>>` |  | Sections of key=value lines, by section name. |
| `env` | `Option<Map<Str, Str>>` |  | KEY=value lines readable by POSIX shells. |
| `mode` | `Int where 0..=0o777` | `0o644` | The permission bits. |
| `owner` | `Ref<User>` | `root` | The owning user. |
| `group` | `Ref<Group>` | `root` | The owning group. |
| `copy` | `Bool` | `false` | Copy the file into /etc instead of linking it through /etc/static. |
| `enable` | `Bool` | `true` | false removes the entry, including a package default or a derived file. |

### `Kernel`

Kernel modules to load, blacklist and configure.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `modules` | `List<Str where matches(r"[A-Za-z0-9_-]+")>` | `[]` | Loaded during boot, after the root filesystem is mounted, in order. |
| `blacklist` | `Set<Str where matches(r"[A-Za-z0-9_-]+")>` | `[]` | Never loaded automatically. |
| `options` | `Map<Str, Str>` | `{}` | Module parameters, for example { snd_hda_intel = "power_save=1" }. |

## Packages, repositories and views

**`Channel`**: The channel of a repository a client follows. `current | testing`

### `Repo`

A package repository. Block label: the repository name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `url` | `Str where matches(r"(https\|file\|http)://.*")` |  | The primary base URL: https://, file://, or http:// with allow_http. |
| `mirrors` | `List<Str>` | `[]` | More base URLs serving the same content, tried in order after url. |
| `key` | `Str where nonEmpty` |  | The root public key: the base64 line of a minisign public key. |
| `keys` | `List<Str>` | `[]` | More accepted root keys, used while a root key is replaced. |
| `channel` | `Channel` | `current` | The channel to follow. |
| `priority` | `Int` | `0` | Higher wins when several repositories offer a package. |
| `enable` | `Bool` | `true` | Whether the repository is used. |
| `allow_http` | `Bool` | `false` | Permit http:// URLs. |

### `Gc`

Retention policy for generations and automatic collection.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `keep_generations` | `Int where 2..=1000` | `10` | Always keep at least this many newest generations. |
| `keep_days` | `Int where 0..=3650` | `30` | Also keep every generation younger than this many days. |
| `auto` | `Bool` | `true` | Run gc after every successful apply. |
| `keep_downloads` | `Bool` | `false` | Keep downloaded package files after unpacking. |

**`ViewBase`**: The only base a view can inherit from. `inherit = none` starts empty. `system`

### `View`

A view besides the system view. Block label: the view name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `inherit` | `Option<ViewBase>` | `system` | system: start from the system's package requests. none: start empty. |
| `packages` | `Set<Pkg>` | `[]` | Package requests, replacing system requests of the same name. |
| `alternatives` | `Map<Str, Pkg>` | `{}` | Group name to chosen member, overriding the system's choice. |
| `users` | `Set<Str>` | `[]` | Users allowed to enter the view, in addition to root. |
| `groups` | `Set<Str>` | `["wheel"]` | Members of these groups may enter the view. |

## Boot and storage

**`Loader`**: The boot loader. `limine`

**`Microcode`**: Which CPU microcode the initramfs carries. `auto | intel | amd | both | off`

### `Initrd`

Settings for the initramfs.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `modules` | `Set<Str>` | `[]` | Extra modules, by name. |
| `hostonly` | `Bool` | `true` | Detect the needed modules from the machine running the assembly. |
| `shell` | `Bool` | `true` | Include the emergency shell. |
| `root_wait` | `Int where 1..=600` | `30` | Seconds to wait for boot devices. |

### `Boot`

Boot loader, kernel and boot counting.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `loader` | `Loader` | `limine` | The boot loader. |
| `esp` | `Str` | `"/boot"` | The mountpoint of the EFI system partition. It needs an `fs` entry of type vfat. |
| `kernel` | `Pkg` | `linux` | The package providing the kernel. |
| `cmdline` | `List<Str>` | `["quiet", "loglevel=3"]` | The kernel command line. |
| `timeout` | `Option<Int where 0..=600>` | `3` | The menu timeout in seconds. none waits forever. |
| `editor` | `Bool` | `false` | Allow editing entries at the boot menu. |
| `tries` | `Int where 0..=10` | `3` | Boot attempts before falling back. 0 disables boot counting. |
| `microcode` | `Microcode` | `auto` | Which CPU microcode the initramfs carries. |
| `initrd` | `Initrd` | `Initrd {}` | Settings for the initramfs. |

**`FsType`**: A filesystem type. `other` names one that is not listed. `ext2 | ext3 | ext4 | xfs | btrfs | f2fs | vfat | tmpfs | nfs | nfs4 | other(name: Str where matches(r"[a-z0-9.]+"))`

### `Fs`

A mounted filesystem. Block label: the mountpoint.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `device` | `Option<Str>` | `none` | Required unless fstype is tmpfs. |
| `fstype` | `FsType` |  | The filesystem type. |
| `options` | `List<Str>` | `[]` | Mount options. Empty means the defaults. |
| `fsck` | `Option<Bool>` | `none` | none: true for ext2, ext3, ext4, f2fs and vfat, false otherwise. |

### `Swap`

A swap device. Block label: the device.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `priority` | `Option<Int where -1..=32767>` | `none` | The swap priority. |

**`LuksOption`**: An option of an unlocked LUKS device. `discard | no-read-workqueue | no-write-workqueue`

### `Luks`

A LUKS device unlocked in the initramfs. Block label: the device-mapper name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `device` | `Str` |  | UUID=... or /dev/... |
| `options` | `Set<LuksOption>` | `[]` | Options for the unlocked device. |

## Services

**`Init`**: The init system. Only runit is implemented. `runit | s6 | dinit`

**`ServiceKind`**: How a service is supervised. `longrun | oneshot`

**`Ready`**: When a service counts as ready. `immediate | notify(fd: Int where 3..=1023) | check(cmd: List<Str> where nonEmpty, interval: Int where 1..=3600 = 1, timeout: Int where 1..=86400 = 60)`

**`Log`**: Where the output of a service goes. `logged | discard | console`

**`Restart`**: Whether the supervisor restarts a longrun after it exits. `always | on-failure | never`

**`Signal`**: The signal that asks a service to stop. `TERM | INT | HUP | QUIT | USR1 | USR2`

**`OnChange`**: What live reconciliation does when only the spec of a service changed. `restart | rerun | ignore | reboot`

### `LogRotation`

How a logged service's output is rotated.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `size` | `Int where 4096..=1_073_741_824` | `1_000_000` | Bytes per file before rotation. |
| `count` | `Int where 1..=1000` | `10` | Rotated files kept. |

### `Dir`

A directory the launcher creates before every start.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `path` | `Str where matches(r"[A-Za-z0-9._@+-]+(/[A-Za-z0-9._@+-]+)*")` |  | Relative to /var/lib for state_dirs, or to /run for runtime_dirs. |
| `mode` | `Int where 0..=0o7777` | `0o755` | The permission bits. |
| `owner` | `Option<Ref<User>>` | `none` | none: the service user. |
| `group` | `Option<Ref<Group>>` | `none` | none: the primary group of owner, or the service's group. |

### `Limits`

Resource limits, each applied as the soft and the hard limit.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `nofile` | `Option<Int where 1..=1_073_741_816>` | `none` | Open files. |
| `nproc` | `Option<Int where 1..=4_194_304>` | `none` | Processes of the service user. |
| `core` | `Option<Int where 0..=1_099_511_627_776>` | `none` | Core dump size in bytes. 0 disables core dumps. |
| `memlock` | `Option<Int where 0..=1_099_511_627_776>` | `none` | Locked memory in bytes. |

### `Service`

A system service. Block label: the service name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `description` | `Str` | `""` | A one-line description. |
| `kind` | `ServiceKind` | `longrun` | How the service is supervised. |
| `enable` | `Bool` | `true` | Whether the service is part of the generation. |
| `exec` | `List<Str>` | `[]` | Longrun: the command to supervise, as argv. |
| `up` | `List<Str>` | `[]` | Oneshot: the command run on start, as argv. |
| `down` | `List<Str>` | `[]` | Oneshot: the command run on stop, as argv. |
| `user` | `Ref<User>` | `root` | The account the service runs as. |
| `group` | `Option<Ref<Group>>` | `none` | none: the primary group of user. |
| `groups` | `Set<Ref<Group>>` | `[]` | Supplementary groups. |
| `env` | `Map<Str, Str>` | `{}` | Environment variables. |
| `cwd` | `Str` | `"/"` | The working directory. |
| `umask` | `Int where 0..=0o777` | `0o022` | The file mode creation mask. |
| `after` | `List<Ref<Service>>` | `[]` | Services that must be ready before this one starts. |
| `ready` | `Ready` | `immediate` | When the service counts as ready. |
| `log` | `Log` | `logged` | Where its output goes. |
| `rotate` | `LogRotation` | `LogRotation {}` | How its log is rotated. |
| `state_dirs` | `List<Dir>` | `[]` | Directories under /var/lib to create before every start. |
| `runtime_dirs` | `List<Dir>` | `[]` | Directories under /run to create before every start. |
| `restart` | `Restart` | `always` | The restart policy of a longrun. |
| `restart_delay` | `Int where 0..=3600` | `1` | Seconds to wait before a restart. |
| `stop_signal` | `Signal` | `TERM` | The signal that asks the service to stop. |
| `stop_timeout` | `Int where 1..=3600` | `10` | Seconds between the stop signal and SIGKILL. |
| `limits` | `Limits` | `Limits {}` | Resource limits. |
| `on_change` | `Option<OnChange>` | `none` | none: restart for longruns, rerun for oneshots. |
| `early` | `Bool` | `false` | Member of the early boot graph. Reserved for the base part. |
| `packages` | `Set<Pkg>` | `[]` | Packages added to the system view while the service is enabled. |

### `Getty`

The terminals that get a login prompt.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `ttys` | `List<Str where matches(r"tty(S\|USB)?[0-9]+")>` | `["tty1", "tty2"]` | Terminal names. Each gets a service named getty-&lt;tty&gt;. |

## Security

### `Privileged`

A file that keeps setuid, setgid or file capabilities in the active generation. Block label: the absolute path.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `enable` | `Bool` | `true` | Whether the entry applies. |
| `owner` | `Ref<User>` | `root` | The owning user. |
| `group` | `Ref<Group>` | `root` | The owning group. |
| `mode` | `Int where 0..=0o7777` | `0o4755` | The mode, including setuid and setgid bits. |
| `caps` | `Option<Str>` | `none` | File capabilities in cap_from_text(3) syntax, such as "cap_setuid=ep". |

### `Security`

Security settings.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `privileged` | `Map<Str, Privileged>` | `{}` | Privileged files. Block label: the absolute path. Written as `privileged` blocks. |


## The base part

Every system includes a base part before its own file. It declares:

- The static system accounts, with the same ids on every Laft system: `root` (0), the groups `tty` (5), `disk` (6), `lp` (7), `kmem` (9), `wheel` (10), `cdrom` (11), `mail` (12), `dialout` (18), `floppy` (19), `audio` (20), `video` (21), `utmp` (22), `input` (23), `kvm` (24), `render` (25), `tape` (26), `sgx` (27), `shadow` (41), `plugdev` (42) and `users` (100), the service accounts `messagebus`, `sshd`, `dhcpcd`, `chrony`, `polkitd`, `rtkit`, `uuidd`, `dnsmasq`, `avahi`, `tss` and `log` (30 to 40), and `nobody` with `nogroup` (65534).
- The early boot services `laft-activate`, `kernel-fs`, `kmod`, `udevd`, `udev-trigger`, `clock`, `hostname`, `mount-fs`, `swap`, `random-seed`, `sysctl`, `console` and `early`, then `turnstiled` and `boot-complete`.
- `/etc/turnstile/turnstiled.conf`, which follows `session.rundir`.
- A tmpfs on `/tmp`.
- The `core` repository, with a placeholder URL in the reserved `.invalid` domain until an official repository is published.
- The privileged programs `laft-enter`, `doas`, `passwd`, `fusermount3` and `unix_chkpwd` (setuid or setgid), and `newuidmap` and `newgidmap` (with file capabilities).
- Kernel hardening sysctls, such as `kernel.kptr_restrict = 1`, `kernel.yama.ptrace_scope = 1`, `fs.protected_symlinks = 1` and `net.ipv4.conf.all.rp_filter = 2`.

## The `std` catalog

Configurations import catalog modules with `import "std/services" as services` and include their parts with `include services.sshd()`.

| Part | What it adds |
|---|---|
| `services.sshd(port = 22)` | The OpenSSH server with `sshd-keygen`, which creates missing host keys, and an `sshd_config` that allows public keys only and no root login. |
| `services.dbus()` | The D-Bus system message bus. |
| `services.seatd()` | The seat management daemon. Members of `video` may open a seat. |
| `services.elogind()` | The elogind login manager, with D-Bus. It manages `/run/user/<uid>` instead of turnstile. |
| `services.dhcpcd()` | The dhcpcd DHCP client for every interface. |
| `services.iwd()` | The iwd wireless daemon, with D-Bus. |
| `services.chronyd()` | The chrony time daemon, running as `chrony`. |
| `services.nftables()` | Firewall rules loaded from `/etc/nftables.conf` on start and flushed on stop. |
| `desktop.wayland(user, compositor = niri, terminal = foot, fonts = [])` | From `std/desktop`: a Wayland desktop for one user with a compositor (`sway`, `niri` or `river`), seatd, PipeWire, portals and fonts. The user joins `video`, `audio` and `input`. |

The packages behind most of these parts have no recipes yet. See [Status and roadmap](../guide/status).
