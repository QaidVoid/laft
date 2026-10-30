# Diagnostic codes

<p class="lead">Every problem Laft reports has a stable code. The letter says which part found it, and <code>laft explain &lt;code&gt;</code> prints the explanation and fix for the codes of the system configuration and the resolver.</p>

| Prefix | Found by |
|---|---|
| `E` | Stav, while it parses, type checks and evaluates a file. See the [Stav error codes](https://github.com/QaidVoid/stav/blob/main/docs/reference/error-codes.md). |
| `C` | `laft`, when it validates and activates the system configuration. |
| `S`, `W0xx` | The resolver, when it chooses the packages of a view. |
| `R` | `laft-build`, when it checks recipes and stage files. |
| `B` | `laft-build`, when it fetches, builds and packages. |
| `W1xx` | `laft-build` post-processing, for things it accepts but reports. |
| `T` | `laft-build plan`, for the reason a package is rebuilt. |

```console
$ laft explain c0901
C0901: A package reference does not resolve against the synced indexes.

fix: Fix the name, or run `laft repo sync` when the package is new.
```

## System configuration

Stav checks types, refinements, required fields and references. After evaluation, `laft` applies these rules. `C0409` to `C0413` come up while the accounts are activated rather than while the file is checked.

| Code | Problem | Fix |
|---|---|---|
| `C0405` | A user or group name is not valid. | Use a name of lowercase letters, digits, `_` and `-` that starts with a letter, at most 32 characters. |
| `C0406` | A user sets both `password_hash` and `password_file`. | Keep one of them. |
| `C0407` | An explicit uid or gid lies in a range reserved for the static system accounts. | Leave the id out, so that Laft allocates one, or pick one from 1000 on. |
| `C0408` | The id of `root` or of a static account was changed. | Remove the id. Static accounts have the same ids on every Laft system. |
| `C0409` | The account databases stayed locked, because another program is editing them. | Finish or stop the other program, such as vipw or passwd, and run the command again. |
| `C0410` | A line of /etc/passwd, /etc/group, /etc/shadow or /etc/gshadow cannot be parsed. Nothing was written. | Fix or remove the line the message names, then run the command again. |
| `C0411` | An account created outside Laft has the id of a declared account. | Remove or renumber the other account, or give the declared one another id. |
| `C0412` | A secret file is missing, not owned by the user running Laft, or readable by its group or others. | Create the file, owned by that user, with mode 0600 or stricter. |
| `C0413` | A password file does not hold a valid password hash. | Write a hash made by mkpasswd or openssl passwd into the file. |
| `C0414` | Two declared accounts have the same id. | Give one of them another id, or leave both out. |
| `C0415` | A regular user would take a system group as its primary group. | Set the user's `group` explicitly. |
| `C0420` | A `file` path is not an absolute, normalized path below `/etc/`. | Write it as `/etc/<path>` without `.`, `..` or repeated slashes. |
| `C0421` | A `file` path is reserved for Laft, such as `/etc/static` or an account database. | Configure the setting the file belongs to instead. |
| `C0422` | An enabled `file` does not set exactly one of `text`, `source`, `link`, `json`, `toml`, `yaml`, `ini` and `env`. | Keep exactly one content field. |
| `C0425` | A path is both a file and the directory of another entry. | Move one of the files. |
| `C0426` | Structured content cannot be written in the format of its file. | Use values the format can express, for example no null in TOML. |
| `C0427` | An `env` key is not a variable name. | Use letters, digits and `_`, not starting with a digit. |
| `C0431` | A sysctl key is malformed. | Write it with dots, such as `net.ipv4.ip_forward`. |
| `C0433` | A kernel module is both loaded and blacklisted. | Remove it from one of `kernel.modules` and `kernel.blacklist`. |
| `C0434` | A sysctl value is not an integer, string or boolean. | Write the value the kernel expects. |
| `C0450` | The init backend is not available in this version. | Remove the line or write `init = runit`. |
| `C0490` | A value cannot be written into the file it belongs to. | Remove characters such as newlines or `:` that the file format cannot hold. |
| `C0491` | No free id is left for an account. | Set an explicit id. |
| `C0492` | A reference names an account that is not declared. | Declare the account or fix the name. |
| `C0801` | A repository URL has an unsupported scheme, or uses `http://` without `allow_http`. | Use `https://` or `file://`, or set `allow_http = true` for a trusted network. |
| `C0802` | A repository key is not a minisign public key. | Paste the base64 line of the repository's `.pub` file. |
| `C0803` | A repository name is not a package name. | Use lowercase letters, digits and `-`. |
| `C0901` | A package reference does not resolve against the synced indexes. | Fix the name, or run `laft repo sync` when the package is new. |
| `C0902` | An alternatives group is not a name, or a member is not a bare package name. | Write `alternatives.vi = neovim` without version or variant. |
| `C0903` | A view name is not a package name, or is `system` or `initrd`. | Rename the view. |
| `C0904` | A view allows a user or group that is not declared. | Declare the account or remove it from the view. |
| `C1101` | No root filesystem is declared. | Add `fs "/" { device = ..., fstype = ... }`, usually in hardware.stav. |
| `C1102` | A mountpoint is not an absolute, normalized path. | Write it as `/path` without `.` or `..`. |
| `C1103` | A mountpoint is managed by Laft, such as `/usr` or `/run`. | Remove the filesystem. |
| `C1104` | The root filesystem type cannot be mounted by the initramfs. | Use ext4, xfs, btrfs or f2fs. |
| `C1105` | A device of the root, `/laft`, the ESP or a LUKS block cannot be resolved by the initramfs, or a LUKS name is invalid. | Use `UUID=`, `PARTUUID=`, `LABEL=` or a `/dev/mapper/` name. |
| `C1106` | The root filesystem is mounted read-only. | Remove `ro` from its options. |
| `C1107` | `boot.esp` has no `vfat` filesystem. | Declare the ESP as `fs "/boot" { fstype = vfat, ... }`. |
| `C1108` | A `/dev/mapper/<name>` device has no `luks <name>` block. | Declare the LUKS device. |
| `C1109` | A filesystem has no device. | Set `device`. |
| `C1110` | A swap device is not a path or a `UUID=`, `LABEL=`, `PARTUUID=` or `PARTLABEL=` tag. | Fix the device. |
| `C1201` | A service name is invalid or reserved. | Use another name. |
| `C1202` | A getty terminal is invalid or repeated. | List each terminal once, such as `tty1`. |
| `C1203` | A service setting does not fit its kind. | Use `exec` for longruns and `up` and `down` for oneshots. |
| `C1204` | A command or path is not absolute. | Write the full path, such as `/usr/bin/sshd`. |
| `C1205` | A service depends on a disabled service, or an early service on a later one. | Enable the dependency or remove it from `after`. |
| `C1206` | Services depend on each other in a cycle. | Remove one of the `after` entries. |
| `C1207` | An environment variable cannot be passed to a program. | Use a variable name without `=` and a value without NUL. |
| `C1208` | Services share a directory with different owners or modes. | Give them the same owner and mode, or separate directories. |
| `C1209` | A service package is not a bare package name. | Write the name without version or variant. |
| `C1210` | `early` is set on a service outside the base part. | Remove `early`. Only the base part defines the early boot graph. |
| `C1301` | A privileged file lies outside `/usr/bin` and `/usr/lib`. | Grant privileges only to programs. |
| `C1302` | A privileged file grants nothing. | Set a setuid or setgid mode or capabilities, or remove the entry. |
| `C1303` | The capability text is invalid. | Write it like `cap_net_raw=ep`. |

## Resolution

The resolver reports why it could not choose the packages of a view, with the chain of requests and dependencies that led there.

| Code | Problem | Fix |
|---|---|---|
| `S001` | No candidate matches a request. | Check the name, version prefix and variant with `laft search`, or sync the indexes. |
| `S002` | Requests for the same package disagree on the variant or the version prefix. | Request the package once per view. |
| `S003` | Nothing satisfies a dependency of a selected package. | The repository is incomplete. Sync again later, or pin the package to an older version. |
| `S004` | Several packages provide a dependency and nothing says which one to use. | Request the provider you want in the view. |
| `S005` | The constraints on a package leave no candidate. | Relax one of the listed constraints, for example a version prefix. |
| `S006` | Two selected packages conflict, or one replaces the other. | Remove one of the requests that pull them in. |
| `S007` | The only providers of a dependency are selected in a version or variant that does not provide it. | Request the provider in the version that provides it. |
| `S008` | Several repositories of the same, highest priority list a package. | Give the repositories different priorities. |
| `S009` | The resolver gave up after too many passes. | Request more packages explicitly to narrow the search. |
| `S010` | An installed package was yanked and nothing else satisfies what needs it. | Wait for a fixed package or pin another version. |
| `W001` | A requested package was renamed, and the request now selects the package that replaces it. | Update the configuration to the new name. |
| `W002` | An installed package is in no repository any more and is kept. | Remove the request or find a replacement. |
| `W003` | An installed version was yanked, so the resolution moves to another one. | Nothing to do. The reason is shown with the warning. |

## Recipes

`laft-build lint` and every build check recipes after Stav has evaluated them.

| Code | Problem |
|---|---|
| `R001` | A recipe that declares a package other than its directory name, or none. |
| `R002` | A path literal outside the package's recipe directory and `common/`, or a path or import that leaves the recipe repositories. |
| `R003` | A recipe directory, subpackage or alternative group whose name is not a package name. |
| `R004` | A repository without exactly one `recipe_repo` block, listed twice, or extending one not listed before it. |
| `R006` | A path literal naming a file that cannot be read. |
| `R010` | A source URL that is not `https://`. |
| `R011` | A source that sets both or neither of `url` and `git`, an empty `url` list, or `file` or `extract` on a git source. |
| `R012` | A URL source without `sha256`. |
| `R013` | A git source without `commit`. |
| `R014` | A vendor block whose `source` names no source. |
| `R015` | A `dest`, `dir`, `subdir`, file name, license file, or `setuid`, `caps` or alternative path that is not a plain relative path. |
| `R016` | An empty package with sources or a style other than `custom`, or a package without sources that is not empty. |
| `R017` | The `cargo` or `go` style without the matching `vendor` block. |
| `R018` | A source label that is not a plain name of letters, digits, `-`, `_` and `.` that does not start with `.`. |
| `R019` | An alternative whose `subpackage` is neither an explicit nor an automatic subpackage of the package. |
| `R030` | An `env` key that does not match ``A-Z_``A-Z0-9_`*`. |
| `R031` | An `env` key that the builder sets itself, unless it is set with `force(...)`, and always `JOBS` and `MAKEFLAGS`. |
| `R050` | An `update` block without exactly one of `github`, `gitlab`, `anitya` and `url`, or with `pattern` and `url` not set together. |
| `R060` | A stage file without exactly one stage, a stage name that does not start with a lowercase letter or digit followed by lowercase letters, digits and dashes, a step that sets both or neither of `packages` and `script`, a step name used twice, a search directory that is not a plain relative path, a missing recipe repository, a package that builds nothing in the variant named, a step or search directory that a stage building into the store cannot have, a view script outside such a stage or without packages, a view named by a step that does not run in one, or a stage given a root it cannot have or missing one it needs. |

Between recipes, and while planning:

| Code | Problem |
|---|---|
| `R020` | A reference to a package of the run in a variant it does not build. |
| `R021` | A reference that no recipe of the run and no published package satisfies. |
| `R022` | More than one recipe of the run for the same package and variant, reported once with their repositories. |
| `B010` | Dirty items that depend on each other in a cycle. |

## Sources

Fetching and unpacking sources:

| Code | Problem |
|---|---|
| `B020` | A location served a file whose SHA-256 hash differs from the recipe's. |
| `B021` | An archive entry or link that leads outside its destination. |
| `B022` | No URL and no mirror has the file. |
| `B023` | A local file that cannot be read or written. |
| `B024` | An archive that cannot be read, or a git source, which this version cannot fetch. |

## Builds

| Code | Problem |
|---|---|
| `B030` | A phase, or a stage script inside the root, that ran longer than the timeout. |
| `B031` | A build root with less than 10 GiB free. |
| `B032` | A build root whose lock another run holds. |
| `B040` | A phase or tool that failed. |
| `B041` | An installation or unpacking into a directory outside bootstrap mode. |
| `B042` | A stage root whose top-level entries differ from Laft's layout, or a `laft-build` that is not a static ELF executable for a step inside the root. |
| `B043` | A payload path that is a directory in a package and something else in the system it is unpacked into, or the other way round. |
| `B044` | A recipe file that changed since evaluation or cannot be read, a license file that is missing or outside the source tree, a build record, lock record, stage record or reproducibility report that cannot be read or serialized, an install or stage record that is neither a build key line nor a build key record of a format this version reads, or a `/proc/meminfo` without a `MemAvailable` line. |
| `B050` | A build view that the build-side store cannot satisfy. |
| `B060` | A reproducibility check of a package without a successful build. |
| `B061` | A reproducibility check with a package that differs or could not be checked. |
| `B1xx`, `R040`, `W1xx` | A payload that breaks a post-processing rule, see [post-processing](#post-processing). |

## Post-processing

After the `install` phase, post-processing turns the payload into packages. Each step that fails stops the build with its code. The `W` codes are warnings.

| Code | Problem |
|---|---|
| `B101` | The install phase produced no files. |
| `B102` | A path cannot move into the `usr` layout, because the target is not a directory or both exist. |
| `B103` | A path outside `usr/` and `etc/`. |
| `B104` | A shared object in a `static` build. |
| `B105` | A dynamically linked program in a `static` build. |
| `B106` | A setuid file that the recipe does not declare in `setuid`. |
| `B107` | An ELF file that searches libraries in a directory of the build. |
| `B108` | An ELF file built for a machine other than x86-64, outside `usr/lib/firmware`. |
| `B109` | A symlink that points into the build. |
| `B110` | The smoke test failed. |
| `B111` | A path listed in `setuid` or `caps` that is not a regular file of the payload. |
| `B112` | A library link that does not point at the library its soname names. |
| `B113` | An ELF file that needs a library only found in a private directory, without a `$ORIGIN` run path. |
| `B114` | An `ownership` entry that is not `user:group`, or names an account that is not a static system account. |
| `B115` | A subpackage whose name is not a valid package name. |
| `B116` | A dependency or capability that is not valid. |
| `R040` | An alternative that names a subpackage the build did not emit, or a path the package does not contain. |
| `W101` | The payload has no license file in `usr/share/licenses/`. |
| `W110` | Static archives were removed. Set `keep_archives` to ship them. |
| `W111` | An ELF file searches libraries in a directory not relative to `$ORIGIN`. |
| `W112` | A script runs an interpreter that is not detected as a dependency. Declare its package in `deps`. |
| `W113` | Files that views generate themselves were removed. |
| `W114` | An ELF file needs a library that no package of the build view provides. |
| `W115` | An ELF file that could not be inspected was skipped. |
| `W120` | A symlink points outside the system directories. |
| `W121` | A file contains `/build/`, which is probably a leaked build path. |
| `W122` | A file was installed writable by everyone. |

## Rebuild triggers

`laft-build plan` gives each build the reasons it is needed.

| Code | Trigger |
|---|---|
| `T1` | The plan hash differs from the latest build. Recipe edits, schema changes and style template changes do this. |
| `T2` | The hash of a source, a vendor tarball, a patch or another file the recipe names changed. |
| `T3` | A link dependency no longer provides a soname or a symbol that the latest build imports from it. |
| `T4` | A `static` package inside a `static` build has a new store id. |
| `T5` | The `abi_epoch` of a link dependency changed. |
| `T6` | The toolchain epoch of the recipe repositories changed. |
| `T7` | There is no previous successful build. |
| `T8` | A rebuild was requested. |
