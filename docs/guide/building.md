# Building packages

<p class="lead"><code>laft-build</code> turns recipes into packages. Every build runs in a sandbox with no network, whose <code>/usr</code> is assembled from packages that were themselves built this way. This page covers everyday builds, the reproducibility check, and how the whole base system is bootstrapped from source.</p>

## The build root

Everything `laft-build` keeps lives in one **build root**: the build-side store, the source cache, the build database, the output directory with package files and their build records, working trees and logs. It is `--root <dir>`, or `$LAFT_BUILD_ROOT`, or `~/.local/share/laft-build`. Recipe repositories come from `--recipes <dir>`, repeated for overlays, or from the colon separated list in `$LAFT_RECIPES`.

```sh
export LAFT_BUILD_ROOT=~/.cache/laft-build
export LAFT_RECIPES=~/laft-recipes
```

A command that writes to the build root holds its lock for the whole run, so a second run on the same root fails at once with exit status 4. `eval`, `plan`, `lint` and dry runs only read and take no lock. Builds need 10 GiB free in the build root.

## Building

```sh
laft-build plan zlib            # what would be built, and why
laft-build build zlib           # build it, then whatever its change makes stale
laft-build build zlib:static    # one variant
```

`plan` lists the builds in order, each with the reason it needs one:

```text
zlib  T8 forced
```

| Code | Rebuild trigger |
|---|---|
| `T1` | The plan hash differs from the latest build. Recipe edits, schema changes and style template changes do this. |
| `T2` | The hash of a source, vendor tarball, patch or other file the recipe names changed. |
| `T3` | A link dependency no longer provides a soname or symbol that the latest build imports from it. |
| `T4` | A `static` package inside a `static` build has a new store id. |
| `T5` | The `abi_epoch` of a link dependency changed. |
| `T6` | The toolchain epoch of the recipe repositories changed. |
| `T7` | There is no previous successful build. |
| `T8` | A rebuild was requested with `--rebuild`. |

A rebuild does not cascade blindly. When a library is rebuilt, the packages that link against it are rebuilt only if the library stopped providing a symbol they import, or if a static library they include changed. Otherwise the run stops at the unchanged output.

Each build fetches its sources into a cache addressed by SHA-256 and verifies them first, unpacks them with rules that keep every archive entry inside its destination, and assembles a **build view**: a `/usr` from the build-side store holding the build base, the recipe's declared dependencies and their runtime dependencies. Every phase then runs in a sandbox with new user, mount, PID, IPC, UTS and network namespaces, no capabilities, `no_new_privs`, the build view as `/usr` and writable `/build` and `/out` directories. Only the fetch keeps the network. The resulting packages enter the build-side store, where the next builds find them.

When a build fails, `--shell-on-failure` opens a shell in its sandbox, and `--keep-work` keeps the working tree. `laft-build shell <package> --after configure` runs the phases up to one phase and opens a shell there, which is the quickest way to work on a recipe.

## Checking reproducibility

```sh
laft-build check-repro zlib
laft-build check-repro          # every package with a successful build
```

`check-repro` rebuilds the latest builds somewhere else, with half the jobs, in the build view of the first build and without check phases. It compares the store ids of the outputs and the package files byte for byte. Every package is checked even when an earlier one differs, and a package that differs is listed with the files that differ and fails the run with `B061`:

```text
checking zlib
zlib: reproducible
checking zstd
zstd: reproducible
89 of 89 reproducible
```

Builds are deterministic by construction. The sandbox has fixed paths, `SOURCE_DATE_EPOCH` is fixed, debug paths are remapped, archives are written in a canonical order, and every manifest and record is canonical JSON.

## The bootstrap

A distribution built from source has a chicken-and-egg problem: compiling GCC and glibc needs a GCC and a glibc. Laft solves it in four stages, each a Stav **stage file** in `laft-recipes/bootstrap/` that `laft-build stage` runs. A stage is a list of steps, each of which builds packages in order or runs a checkpoint script. Finished steps are skipped, so a failed run continues where it stopped, and a step that changed runs again with every step after it.

<Stages />

### Stage 0 and stage 1: out of the host

Stage 0 builds without root, from any Linux system, with a statically linked `laft-build`. It builds a cross binutils, GCC and glibc, then the temporary tools, into a new root with Laft's layout, and finishes the temporary system inside that root, entered as a chroot through an unprivileged user and mount namespace:

```sh
laft-build --root ~/.cache/laft-bootstrap/build \
  stage bootstrap/stage0.stav --root ~/.cache/laft-bootstrap/root --jobs 4
```

Stage 1 builds the whole base system from `recipes/` inside that root, with the temporary toolchain. Each build makes real Laft packages, which stay in the build root's output directory and are unpacked into the root, so that each build after it uses them instead of the temporary tools:

```sh
laft-build --root ~/.cache/laft-bootstrap/build \
  stage bootstrap/stage1.stav --root ~/.cache/laft-bootstrap/root --jobs 4
```

Its checkpoints check that the new toolchain compiles and runs C and C++ against the new glibc and libstdc++, and that the system has what a Laft machine boots with: dash as `/bin/sh`, the kernel and its modules, Limine, runit and udev.

### Stage 2: hermetic

Stage 1 packages were built with tools the host provided. Stage 2 rebuilds every recipe in the sandbox. `laft-build seed` adds the stage 1 packages to a new build root's store, where each one stands in for its package only until stage 2 has built its own:

```sh
laft-build --root ~/.cache/laft-bootstrap/stage2 \
  seed ~/.cache/laft-bootstrap/build/out/*.lpkg
laft-build --root ~/.cache/laft-bootstrap/stage2 \
  stage bootstrap/stage2.stav --jobs 4
```

The order follows the declared dependencies, toolchain first, so that every package after GCC is compiled by the stage 2 compiler. Each build sees only the build base and what its recipe declares.

### Stage 3: hermetic again, with tests

Stage 3 seeds a third build root with the stage 2 packages alone, so no package of stage 1 reaches any view. It rebuilds everything again, this time running the check phase of every recipe with its check dependencies, and then checks that the result rebuilds byte for byte:

```sh
laft-build --root ~/.cache/laft-bootstrap/stage3 \
  seed ~/.cache/laft-bootstrap/stage2/out/*.lpkg
laft-build --root ~/.cache/laft-bootstrap/stage3 \
  stage bootstrap/stage3.stav --jobs 4
laft-build --root ~/.cache/laft-bootstrap/stage3 --recipes . check-repro
```

The packages of a finished bootstrap boot in the virtual machine test with `tests/vm/run --self-built <dir>`, as described in [Try it in QEMU](./try-it#the-self-built-system).

::: tip Resources
The bootstrap takes hours. Build with at most four jobs on a machine shared with other work. `laft-build stage` waits for `--min-free-gib` of available memory, 6 GiB by default, before each build whose recipe sets `ci { runner = big }`, such as GCC and glibc.
:::

## Publishing

`laft-repo` turns a directory of package files into a repository of static files:

```sh
laft-repo keygen --out keys/online --unencrypted
laft-repo index --repo repo --name core --channel testing --key keys/online.key out/
laft-repo check --repo repo --channel testing
laft-repo promote --repo repo --min-soak 24h --root-key keys/root.pub --check-report report.json --key keys/online.key
laft-repo publish repo /srv/www/laft
```

`index` adds packages to a channel as a new index generation and signs it. `check` runs the repository-wide checks, `promote` copies a channel's packages into another once they have soaked and passed, `yank` withdraws a package with a reason, `keys` maintains the list of online signing keys, and `publish` copies the repository to a web root in an order that is safe for readers. The virtual machine test uses `keygen`, `index` and `publish` to serve its packages. A hosted repository is [planned](./status).
