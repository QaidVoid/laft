# The `laft` command

<p class="lead"><code>laft</code> applies, switches and inspects a Laft system. This page lists every command and option of the current development version, as <code>laft --help</code> and <code>laft &lt;command&gt; --help</code> print them. For how they fit together, see <a href="../guide/everyday">Everyday use</a>.</p>

```text
laft [OPTIONS] <COMMAND>
```

Commands that only read never take the transaction lock and never need root. Commands that write take the lock and need root, except below a root given with `--root`, which also means that nothing is activated on the running system. Results go to standard output, and progress, warnings and errors to standard error. With `--json`, query commands print one JSON document and long-running commands one JSON event per line.

## Global options

Every command accepts these.

| Option | Meaning |
|---|---|
| `--config <PATH>` | The configuration entry point. Defaults to `$LAFT_CONFIG`, else `etc/laft/system.stav` below the root. |
| `--root <DIR>` | Operate on a Laft system mounted at `DIR`. Nothing is activated live. |
| `--json` | Print machine readable JSON. |
| `--color <WHEN>` | `auto`, `always` or `never`. Defaults to `$LAFT_COLOR`, else `auto`. |
| `--ascii` | Use ASCII symbols instead of Unicode. |
| `-v`, `--verbose` | Print more detail. Repeat for even more. |
| `-q`, `--quiet` | Print only errors and the final result. |
| `-y`, `--yes` | Assume yes to confirmations. |
| `--offline` | Never use the network. |
| `--no-wait` | Fail with status 7 instead of waiting for the lock. |
| `--no-pager` | Never page long output. |
| `--message-format <FORMAT>` | How configuration diagnostics are written to standard error: `human`, with source excerpts, or `json`, one object per line. |

## Changing the system

### `laft plan`

Show what `laft apply` would change, without changing anything.

| Option | Meaning |
|---|---|
| `--upgrade` | Ignore the installed versions and take the newest ones. |
| `--refresh` | Sync indexes even if they are fresh. |
| `--exit-code` | Exit with status 12 when the plan contains changes. |

### `laft apply`

Build a generation from the configuration and activate it. It prints the plan and asks before it continues.

| Option | Meaning |
|---|---|
| `--dry-run` | Only show the plan, as `laft plan` does. |
| `--build-only` | Build the generation without activating it or making it the default. |
| `--boot` | Build the generation and make it the default for the next boot. |
| `--upgrade` | Ignore the installed versions and take the newest ones. |
| `--refresh` | Sync indexes even if they are fresh. |
| `--diff` | Print unified diffs of the `/etc` files the new generation changes, once it is built and before it is activated. |
| `-m`, `--message <MESSAGE>` | Store a message with the generation. |
| `--adopt <PATH>` | Move this unmanaged `/etc` file aside instead of failing. Repeatable. |
| `--force` | Build a generation even when nothing changed. |
| `--no-gc` | Skip the automatic garbage collection. |

### `laft upgrade [NAMES]...`

Apply with newer versions of the named packages, or of every package when none is named. Takes every option of `apply`.

### `laft switch <GENERATION>`

Activate an existing generation and make it the default. `--boot` only makes it the default for the next boot.

### `laft rollback`

Switch to the parent of the active generation. `--boot` only makes it the default for the next boot.

### `laft install`

Install Laft onto a target mounted at `--root`. The target may hold nothing but `boot/`, `lost+found/` and `etc/laft/`. `--generate-config` writes a starting `system.stav` and a `hardware.stav` for the target instead.

### `laft hardware`

Generate `hardware.stav` for this machine, or for the target at `--root`: its mounted filesystems, swap, LUKS volumes, CPU microcode and the modules its boot devices need. `--write` writes `etc/laft/hardware.stav` below the root instead of printing it, and never replaces an existing file.

### `laft activate --boot`

Reconcile `/etc` and the accounts with the booted generation. The `laft-activate` early service runs it at every boot.

## Generations and boot

### `laft gens`

| Command | Meaning |
|---|---|
| `laft gens list` | List the generations, with flags for active (`*`), booted (`b`) and default (`d`). |
| `laft gens show <GENERATION> [--packages]` | Show a generation, and with `--packages` every package in it. |
| `laft gens diff <A> [<B>] [--etc]` | Compare two generations. `B` defaults to the active one. `--etc` adds unified diffs of changed `/etc` files. |
| `laft gens rm <GENERATIONS>... [--no-gc]` | Delete generations, then collect garbage unless `--no-gc`. The active, booted and default generations cannot be deleted. |

### `laft boot`

| Command | Meaning |
|---|---|
| `laft boot list` | List the boot menu entries. |
| `laft boot show [<GENERATION>]` | Show the boot record of a generation, by default the booted one. |
| `laft boot log [<GENERATION>]` | Show boot events, optionally only those about one generation. |
| `laft boot set-default <GENERATION>` | Make a generation the boot default without activating it. |
| `laft boot mark-good` | Record the booted generation as good. The `boot-complete` service runs it. |
| `laft boot install` | Render the EFI system partition again from the generations. |

### `laft gc`

Delete generations, views and store entries nothing needs.

| Option | Meaning |
|---|---|
| `--dry-run` | Show what would be deleted. |
| `--explain` | Explain every decision. |
| `--keep-generations <N>` | Keep at least this many newest generations. |
| `--keep-days <D>` | Keep every generation younger than this many days. |

## Checking the system

### `laft verify [PKGS]...`

Verify store entries and generations. Without arguments, the packages of the active generation.

| Option | Meaning |
|---|---|
| `--all` | Verify every store entry and every generation. |
| `--full` | Hash every file. |
| `--links` | Check that generation and view files are the store's files. |
| `--gen <N>` | The generation, by default the active one. |

### `laft doctor`

Run health checks: the layout, `/etc/static`, the runtime state, the `/laft` and generation mounts, the journal, temporary files, free space, indexes, boot files, the static Laft binaries and the clock.

### `laft etc`

| Command | Meaning |
|---|---|
| `laft etc status` | List managed paths that differ from the active generation. |
| `laft etc diff [PATHS]...` | Show how live files differ from the active generation. Every differing path by default. |
| `laft etc reset <PATHS>...` | Restore managed paths. |

### `laft rescue [--gen <N>] [-- <COMMAND>...]`

Enter a Laft system mounted at `--root` from a rescue environment: a private mount namespace with the default generation, or generation `N`, as the system, and the kernel filesystems bound in. Runs root's shell, or the given command. Needs root.

### `laft chroot <DIR> [--gen <N>] [-- <COMMAND>...]`

The same as `laft rescue`, with the mounted system given as `DIR`.

## Packages

| Command | Meaning |
|---|---|
| `laft search <QUERY> [--repo <REPO>] [--all-versions]` | Search packages by name and summary, in one repository or all, listing the newest version or every version. |
| `laft info <PKG>` | Show a package. |
| `laft files <PKG> [-l]` | List the files of a package. `-l` shows type, mode, owner and size. |
| `laft owns <TARGET> [--view <VIEW>]` | Find what provides an absolute path, or a command in `/usr/bin`, in the system view or another view. |
| `laft why <NAME> [--view <VIEW>] [--all]` | Explain why a package is in a view, or in every view. |

## Repositories and views

| Command | Meaning |
|---|---|
| `laft repo sync [NAMES]...` | Sync indexes now. Every enabled repository by default. |
| `laft repo list` | List the configured repositories. |
| `laft repo reset <NAME>` | Forget the accepted index and keys of a repository. |
| `laft view list` | List the views besides the system view. |
| `laft view show <NAME>` | Show a view. |
| `laft view build [NAMES]...` | Rebuild views with the system view pinned. Every view by default. Takes every option of `apply`. |
| `laft enter <VIEW> [-- <COMMAND>...]` | Run a command inside a view, through the setuid `laft-enter`. The login shell by default. |

## Services

| Command | Meaning |
|---|---|
| `laft svc status [NAMES]...` | Show the state of services. Every enabled one by default. |
| `laft svc show <NAME>` | Show a service's spec. |
| `laft svc start <NAMES>...` | Start services until the next apply or boot. |
| `laft svc stop <NAMES>...` | Stop services until the next apply or boot. |
| `laft svc restart <NAMES>...` | Restart services. |
| `laft svc log <NAME> [-n <LINES>] [-f]` | Print a service's log, the last 50 lines by default. `-f` keeps printing new lines. A service whose `log` is `console` or `discard` has no log, and the command fails with a message that says so: its output goes to `/dev/console`, or is thrown away. |

The rendered runit scripts call hidden subcommands of `laft svc` (`exec`, `up`, `down`, `builtin`, `finish`, `logger`, `stop-early` and `ready`). They are Laft's service launcher, not commands to run by hand.

## The configuration

| Command | Meaning |
|---|---|
| `laft check [FILE] [--refresh]` | Check the configuration, resolving its packages, or type check one file on its own without resolving packages. |
| `laft eval [ATTR] [--file <FILE>] [-o json\|toml\|yaml]` | Print the evaluated configuration, or the value at an attribute path such as `services.sshd.exec`. |
| `laft options [PREFIX] [--all]` | List configuration options, optionally only those below a path. The list comes from the schema alone, so no configuration is needed. A `Ref<...>` field names a declared item and is listed as one option. `--all` prints whole doc comments. |
| `laft fmt [PATHS]... [--check] [--stdin]` | Format configuration files, every `.stav` file of the configuration by default. `--check` changes nothing and fails when a file would change. `--stdin` formats standard input to standard output. |
| `laft explain <CODE>` | Explain a diagnostic code, such as `C0901` or `S001`. |
| `laft completions <SHELL>` | Print a shell completion script for `bash`, `zsh` or `fish`. |

## Exit status

| Status | Meaning |
|---|---|
| 0 | Success, including nothing to do. |
| 1 | General failure. |
| 2 | Usage error, or a confirmation nobody can give. |
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

## Environment

| Variable | Meaning |
|---|---|
| `LAFT_CONFIG` | The configuration entry point, when `--config` is not given. |
| `LAFT_COLOR` | When to color output, when `--color` is not given: `auto`, `always` or `never`. |
| `NO_COLOR` | Set to anything, turns off automatic color. |
| `LAFT_PAGER`, `PAGER` | The pager for long output, in that order. `less -FRX` by default. |
