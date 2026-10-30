# The `laft-build` command

<p class="lead"><code>laft-build</code> turns recipes into Laft packages: it evaluates and checks recipes, plans builds, fetches sources, builds in sandboxes, packages the results, checks reproducibility and runs the bootstrap. This page lists every command and option of version 0.1.0. For how they fit together, see <a href="../guide/building">Building packages</a>.</p>

```text
laft-build [OPTIONS] <COMMAND>
```

## Global options

| Option | Meaning |
|---|---|
| `--root <DIR>` | The build root. Defaults to `$LAFT_BUILD_ROOT`, then to `$XDG_DATA_HOME/laft-build` or `~/.local/share/laft-build`. |
| `--recipes <DIR>` | A recipe repository, lowest priority first. Repeat for overlays. Defaults to the colon separated list in `$LAFT_RECIPES`. The `stage` command takes its repository from the stage file instead. |
| `--json` | Print machine readable JSON on standard output, and progress on standard error. |

Global options go before the command. `bootstrap` and `stage` have a `--root` of their own, after the command, which names the system they build rather than the build root:

```sh
laft-build --root ~/.cache/laft-bootstrap/build \
  stage bootstrap/stage0.stav --root ~/.cache/laft-bootstrap/root
```

Package arguments are package names, optionally with a variant, such as `zlib` or `busybox:static`.

## Options of builds

`build`, `shell`, `bootstrap` and `stage` share these:

| Option | Meaning |
|---|---|
| `--no-check` | Skip check phases. |
| `--jobs <N>` | The number of parallel jobs inside a build. Defaults to the number of CPUs. |
| `--timeout-hours <HOURS>` | The longest one phase may run, from 1 to 8760 hours. 6 by default. |
| `--mirror <URL>` | Extra source mirrors, tried after the repositories' own. Repeatable. |

`plan` and `build` share these:

| Option | Meaning |
|---|---|
| `--rebuild` | Build the named packages even when they are up to date. |
| `--use-published <PKG>` | Let the dependents of this package use its published package instead of waiting for its build, which breaks a cycle. |

## Commands

### `laft-build eval <PACKAGE>`

Print the evaluated recipe of a package, with its rendered phase scripts and environment. Without a variant, every variant it builds is printed.

### `laft-build lint [PACKAGES]...`

Type check and evaluate recipes and run the recipe checks. Every package of the repositories by default.

### `laft-build plan [PACKAGES]...`

Show what would be built, in order, with the [rebuild trigger codes](../guide/building#building) `T1` to `T8`. Every package of the repositories by default.

### `laft-build build <PACKAGES>...`

Build packages, then everything the changes make stale.

| Option | Meaning |
|---|---|
| `--keep-work` | Keep the working tree of every build. |
| `--shell-on-failure` | Open a shell in the sandbox of a build that fails. |

### `laft-build fetch <PACKAGES>...`

Fetch and verify the sources of packages. Takes `--mirror`.

### `laft-build shell <PACKAGE>`

Run the phases of a build up to one phase, then open a shell in its sandbox. `--after <PHASE>` names the last phase to run, for example `configure`. Without it, the shell opens before the first phase.

### `laft-build check-repro [PACKAGES]...`

Rebuild the latest builds of packages elsewhere, with half the jobs, in the build view of the first build and without check phases, and compare the store ids of their outputs and their package files byte for byte. Every package is checked even when an earlier one differs or fails to rebuild. Without arguments, every package of the repositories that has a successful build.

### `laft-build bootstrap <PACKAGES>...`

Build packages in the given order without sandbox or build-side store, with the tools of a bootstrap system. The result is not hermetic and must never be published.

| Option | Meaning |
|---|---|
| `--root <DIR>` | The root of the bootstrap system, whose `usr/bin` provides the tools. Inside a chroot of that system, this is `/`. Required. |
| `--install <DIR>` | Install every build straight into this directory, a system under construction, instead of making packages. A package is up to date when its build key matches the one installed there. |
| `--unpack <DIR>` | Make packages as usual, then unpack all but the debug info into this directory, so that later builds there use them. A package is up to date when its build key matches the one unpacked there. |
| `--path <DIR>` | A directory to search for programs before the root's `usr/bin`, such as the `bin` directory of a cross toolchain. Repeat for more, searched in order. |
| `--dry-run` | Print what is up to date and what would be built, without building. |

### `laft-build stage <FILE>`

Run a bootstrap stage: build the packages of a [stage file](./recipe#stage-files) into a root, on the host or inside the root entered as a chroot, or into the build-side store, and run its scripts, in order. Finished steps and the finished packages of a step are skipped, so a failed run continues with the package or script that failed. A step that changed runs again, and so does every step after it. Logs go to `logs/stage/<stage>/` in the build root.

| Option | Meaning |
|---|---|
| `--root <DIR>` | The root the stage builds. It gets Laft's top-level layout. A stage that installs into the store has none, and every other stage needs one. |
| `--min-free-gib <GIB>` | The available memory, in GiB, that a build of a recipe whose CI runner is `big` waits for. 6 by default. |
| `--dry-run` | Print what is up to date and what would run, without running. |
| `--rebuild` | Run every step, even the finished ones. |

### `laft-build seed <FILES>...`

Add package files that another build root made, such as the packages of a bootstrap, to the build-side store as seeds. A seed serves build views until a build here adds a package of the same origin and variant.

### `laft-build gc`

Remove working trees, staging files and old sources. `--sources-older-than <DAYS>` also removes cached sources not used for that many days.

## Locking and disk space

Every command that writes to the build root holds its lock for the whole run, so a second such run on the same build root fails at once. `eval`, `plan`, `lint` and dry runs only read and take no lock. `build`, `check-repro`, `bootstrap` and `stage` need 10 GiB free in the build root and fail with `B031` otherwise.

## Exit status

| Status | Meaning |
|---|---|
| 0 | Success. |
| 1 | A build failed. |
| 2 | A usage or evaluation error. |
| 3 | The machine lacks something builds need, such as unprivileged user namespaces or free disk space. |
| 4 | Another run holds the lock of the build root. |

## `laft-repo`

`laft-repo` maintains package repositories on the build side. Its commands:

| Command | Meaning |
|---|---|
| `keygen` | Create a minisign key pair `PREFIX.key` and `PREFIX.pub`. |
| `index` | Add package files to a channel as a new index generation. |
| `sign` | Sign a channel's index with an online key, or `keys.json` with a root key. |
| `check` | Run the repository-wide checks over a channel's index. |
| `yank` | Withdraw a package from a channel with a reason, as a new generation. |
| `promote` | Copy the packages of one channel into another as a new generation, once the gates hold. |
| `keys` | Create or change `keys.json`, the list of online signing keys. |
| `publish` | Copy a repository to a static target directory in an order that is safe for readers. |

`--now <TIME>` uses a fixed RFC 3339 UTC time instead of the system clock for signing and every age check. Encrypted secret keys are decrypted with the password in `$LAFT_SIGNING_KEY_PASSWORD`, or in the variable `--password-env` names, or one typed at the terminal. `laft-repo` exits with 0 on success, 1 on failure or when a check reports an error, and 2 for a usage error.
