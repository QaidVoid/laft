# Writing recipes

<p class="lead">A recipe describes how one upstream project becomes Laft packages: where its source comes from, what it needs, and how to build it. Recipes are Stav files, so <code>laft-build</code> checks every field before a build starts.</p>

The examples on this page come from the `laft-recipes` repository, which holds the 89 recipes of the base system and the bootstrap.

## The layout of a recipe repository

```text
laft-recipes/
├── recipes.stav                  the repository descriptor
├── recipes/
│   ├── zlib/
│   │   └── package.stav          one package block named zlib
│   ├── coreutils/
│   │   ├── package.stav
│   │   └── patches/
│   │       └── 0001-coreutils-9.7-upstream_fix-1.patch
│   └── ...
└── bootstrap/                    the stage files and the stage 0 recipes
```

`recipes.stav` names the repository and the architectures it builds for:

```stav
recipe_repo core {
  arch = ["x86_64-v3"]
}
```

Each recipe is `recipes/<name>/package.stav` and holds exactly one `package` block labeled `<name>`. Its patches live in `patches/`, numbered in the order they apply, and other files it needs in `files/`. Files shared by several recipes go in `common/`. A recipe refers to its files with path literals such as `./patches/0001-fix.patch`, and may not reach outside its own directory and `common/`.

Several repositories can be stacked: `laft-build --recipes base --recipes overlay` reads both, and a recipe in the later repository merges into the earlier recipe for the same package through Stav merging. An overlay can change one field of a package without copying the rest.

## A first recipe

```stav
package zlib {
  version  = "1.3.1"
  summary  = "A general purpose data compression library"
  homepage = "https://zlib.net"
  license  = [Zlib]

  source main {
    url    = ["https://zlib.net/fossils/zlib-#{self.version}.tar.gz",
              "https://github.com/madler/zlib/releases/download/v#{self.version}/zlib-#{self.version}.tar.gz"]
    sha256 = "sha256:9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
  }

  build make {}
  license_files = ["LICENSE"]

  phases {
    configure = "./configure --prefix=/usr"
  }
}
```

- `version`, `summary`, `license` and `build` are required. `license` lists SPDX identifiers, written as symbols such as `GPL-2.0-only`.
- `source main` is the primary source. Every URL source is `https://` and pinned by `sha256`. The URLs are tried in order, and `laft-build` verifies the hash before the file enters its source cache. `#{self.version}` refers to the recipe's own version, so updating it is a one-line change plus a new hash.
- `build make {}` picks the `make` style, which runs `make`, `make check` and `make DESTDIR="$DESTDIR" install`. zlib's configure script is not autoconf, so the recipe replaces only the `configure` phase.

`std/build` has helpers for common download locations:

```stav
import "std/build" as build

package gzip {
  version  = "1.14"
  summary  = "The GNU compression tool"
  homepage = "https://www.gnu.org/software/gzip/"
  license  = [GPL-3.0-or-later]

  source main {
    url    = build.gnu_mirrors("gzip/gzip-#{self.version}.tar.xz")
    sha256 = "sha256:01a7b881bd220bfdf615f97b8718f80bdfd3f6add385b993dcf6efd14e8c0ac6"
  }

  # zless, which runs less, is made only when configure finds less.
  build_deps = [less]
  deps       = [less]
  # zless is tested with less.
  check_deps = [less]

  build configure {}
}
```

`gnu_mirrors`, `github_archive`, `github_release` and `kernel_org` each return URLs from their parameters, so a recipe passes its version in.

## Build styles

The `build` block chooses a style, and the style renders the phase scripts:

| Style | For |
|---|---|
| `configure` | autoconf projects: `configure` with Laft's standard paths, then `make` |
| `make` | plain makefiles |
| `meson` | meson and ninja, with `auto_features` disabled by default |
| `cmake` | cmake with ninja or make |
| `cargo` | Rust crates, with vendored dependencies |
| `go` | Go modules, with vendored dependencies |
| `python-pep517` | Python packages built with a PEP 517 backend |
| `kernel` | the Linux kernel, from a configuration file |
| `custom` | anything else, with every phase written by hand |

Most styles take arguments. kmod uses meson with its options:

```stav
build_deps = [meson, ninja]
link_deps  = [openssl, xz, zlib, zstd]

# The manual pages need scdoc, which the base system lacks.
build meson {
  options = ["-Dmanpages=false", "-Dzstd=enabled", "-Dxz=enabled", "-Dzlib=enabled"]
}

# The tests build kernel modules, which needs the build tree of a kernel.
check = false
```

`laft-build eval <package>` prints the evaluated recipe with the environment and every rendered phase script. For gzip, the `configure` style renders this:

```sh
#!/bin/sh
set -eu
umask 022
cd "$SRC/"
if false; then cd "$BUILDDIR"; fi
"$SRC//configure" --prefix=/usr --sysconfdir=/etc --localstatedir=/var --bindir=/usr/bin --sbindir=/usr/bin --libdir=/usr/lib --libexecdir=/usr/lib --mandir=/usr/share/man --infodir=/usr/share/info --build=$CBUILD --host=$CHOST --disable-static --enable-shared
```

Laft has a merged `/usr`. Programs go to `usr/bin`, libraries to `usr/lib`, and nothing goes to `usr/sbin`, `usr/lib64` or `usr/local`. The styles pass the right paths, and post-processing moves stray files into place.

## Phases

A build runs these phases in order: `prepare`, `configure`, `build`, `check` and `install`. The `phases` block replaces any of them and adds hooks around them: `pre_configure`, `post_configure`, `pre_build`, `post_build`, `pre_check`, `post_check`, `pre_install` and `post_install`. `subdir` sets the directory below `$SRC` that every phase starts in.

Phases are POSIX `sh`. Each one runs with `set -eu`, so it stops at the first command that fails. Arrays, `pipefail`, `[[ ]]`, `local` and brace expansion are not available. A recipe that really needs bash sets `shell = bash` with a comment that says why, and that makes `bash` a build dependency.

runit has no build system of its own, so its recipe writes every phase:

```stav
build custom {}
license_files = ["runit-2.3.1/package/COPYING"]

# The build reads the compiler and linker commands from conf-cc and
# conf-ld, and package/commands lists the programs.
phases {
  subdir    = "runit-2.3.1/src"
  configure = """
    echo "$CC $CFLAGS" > conf-cc
    echo "$CC $LDFLAGS" > conf-ld
    """
  build     = "make"
  check     = """
    make check
    """
  install   = """
    for program in $(cat ../package/commands); do
      install -Dm755 "$program" "$DESTDIR/usr/bin/$program"
    done
    install -d "$DESTDIR/usr/share/man/man8"
    install -m644 ../man/*.8 "$DESTDIR/usr/share/man/man8/"
    """
}
```

The environment of every phase is fixed. The most useful variables:

| Variable | Value |
|---|---|
| `SRC` | `/build/src`, the unpacked sources |
| `BUILDDIR` | `/build/obj`, for out-of-tree builds |
| `DESTDIR`, `OUT` | `/out`, where `install` puts the payload |
| `RECIPE` | the recipe's directory, for its `files/` |
| `CC`, `CXX`, `CFLAGS`, `CXXFLAGS`, `LDFLAGS` | the compiler and Laft's hardened default flags |
| `CHOST`, `CBUILD` | `x86_64-laft-linux-gnu` |
| `JOBS` | the number of parallel jobs |
| `LAFT_NAME`, `LAFT_VERSION`, `LAFT_REV`, `LAFT_VARIANT` | the package being built |
| `SOURCE_DATE_EPOCH` | `315532800`, so timestamps do not depend on the build time |
| `PATH` | `/usr/bin` |

`env` adds variables, and `cflags`, `cxxflags`, `ldflags`, `rustflags` and `goflags` append to the defaults. `hardening_disable` turns off a default hardening flag such as `fortify` or `pie`.

## Dependencies

A build sees only the build base of `laft-build` and what the recipe declares:

| Field | Present |
|---|---|
| `build_deps` | Tools that run during the build. |
| `link_deps` | Libraries the package compiles and links against. |
| `check_deps` | Packages present only when the `check` phase runs. |
| `deps` | Runtime dependencies that detection cannot see, such as the interpreter of a script. |

Runtime dependencies on shared libraries are not declared. `laft-build` reads every ELF file of the payload and turns its `DT_NEEDED` entries into dependencies, and its sonames into what the package provides. coreutils declares what it builds with and links against, and nothing else:

```stav
patches = [./patches/0001-coreutils-9.7-upstream_fix-1.patch]

# configure refuses to run as root unless told it is safe. util-linux
# ships kill and procps-ng uptime.
env = { FORCE_UNSAFE_CONFIGURE = "1" }
build_deps = [autoconf, automake, gettext, perl]
link_deps  = [acl, attr, gmp, libcap, openssl]

build configure {
  args = ["--enable-no-install-program=kill,uptime"]
}
```

## Tests

The `check` phase runs the project's test suite, and a failing test fails the build. Tests that cannot work in the sandbox are skipped in the recipe, with a comment that says why. Builds run as uid 0 inside a user namespace without any capability, and with no network:

```stav
phases {
  # The tests that run as root, and the tests of chown in gnulib, give
  # files to other users and groups, which the user namespace of the
  # sandbox does not map, or expect root to read, write and remove what
  # its permissions protect, which builds, run as uid 0 without any
  # capability, cannot. They are left out.
  pre_check = """
    for test in chgrp/basic.sh chgrp/default-no-deref.sh ...; do
      sed -i "\|^[[:space:]]*tests/$test[[:space:]]*\\\\\$|d" Makefile
    done
    """
}
```

`check = false` turns the phase off, again with the reason in a comment, and `laft-build build --no-check` skips it for one run. A recipe can also give a `smoke` script, which runs after packaging in a view that holds only the package and its runtime dependencies, so it catches a missing dependency that the build environment hid.

## What a build produces

After `install`, post-processing turns `$DESTDIR` into packages. It moves everything into the `usr` layout, rejects paths outside `usr/` and `etc/`, checks modes and requested privileges, checks the search paths and target machine of every ELF file, strips them and splits their debug info, derives dependencies and ABI digests, and splits the payload into the main package and its `-dev`, `-doc` and `-dbg` subpackages. Each rule that fails stops the build with a code from the [B1xx and W1xx tables](../reference/codes#post-processing).

`split` chooses the automatic subpackages, and `subpackage` blocks add explicit ones from globs:

```stav
split = [dbg, doc]
```

A package that should be static sets `variants = [static]`, and one that builds both sets `variants = [dynamic, static]`. Static builds ship static programs and no shared objects.

## Checking recipes

```sh
laft-build --recipes ~/laft-recipes lint            # check every recipe
laft-build --recipes ~/laft-recipes lint gzip       # check one
laft-build --recipes ~/laft-recipes eval gzip       # print the rendered recipe
laft-build --recipes ~/laft-recipes fetch gzip      # download and verify sources
```

`lint` type checks every recipe and runs the recipe checks: the package name matches its directory, URLs are `https://` and pinned, paths stay inside the recipe, `env` keys are valid and do not override what the builder sets, and more. Each problem has an `R` code, listed in [Diagnostic codes](../reference/codes#recipes). Every field is in the [recipe reference](../reference/recipe).
