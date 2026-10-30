# Recipes and stages

<p class="lead">Every field of a recipe, a recipe repository descriptor and a bootstrap stage file, as the schema in <code>crates/laft-build/stav/prelude/recipe.stav</code> declares it. <a href="../guide/recipes">Writing recipes</a> explains how they are used.</p>

Types are Stav types, as in the [system configuration reference](./system). `Pkg` is a package reference, `Spdx` an SPDX license identifier written as a symbol, and `Path` a path literal relative to the recipe file, which must stay inside the recipe's directory or `common/`.

## What a recipe can import

| Module | Provides |
|---|---|
| `std/build` | `gnu_mirrors(path)`, `github_archive(repo, tag, name)`, `github_release(repo, tag, file)` and `kernel_org(path)`, which build source URLs. |
| `laft/target` | The target of the evaluation: `variant`, `arch`, `cpu`, `triple` and `libc`. Every package is built for `x86_64-v3`. The `dynamic` variant uses the triple `x86_64-laft-linux-gnu` and glibc, and the `static` variant is cross compiled against musl as `x86_64-laft-linux-musl`. |
| `common/...` | Modules of the recipe repository that several recipes share. |

## The `package` block

A recipe file holds one `package` block whose label is the package name and the name of its directory.

### `Package`

One upstream project built into one or more packages. Block label: the package name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `version` | `Str where matches(r"^[A-Za-z0-9._~]+$")` |  | The upstream version. |
| `rev` | `Int where 1..=1000000` | `1` | The Laft revision. |
| `summary` | `Str where matches(r"^[^\n]{0,79}[^.\n]$")` |  | One line of at most 80 characters, without a final period. |
| `description` | `Option<Str>` |  | A longer description. |
| `homepage` | `Option<Str where matches(r"^https://.*")>` |  | The project's web site. |
| `license` | `List<Spdx> where nonEmpty` |  | Every SPDX license or exception that applies. |
| `license_files` | `Option<List<Str>>` |  | License files relative to $SRC. none: every top level COPYING*, LICENSE*, LICENCE*, NOTICE* or COPYRIGHT* file. |
| `maintainers` | `List<Str>` | `[]` | Free form, usually "Name &lt;email&gt;". |
| `variants` | `Set<Variant> where nonEmpty` | `[dynamic]` | The variants this recipe builds. |
| `arch` | `Option<Set<Str>>` |  | The architectures to build for. none: every architecture of the repository. |
| `empty` | `Bool` | `false` | A meta package without payload. |
| `sources` | `Map<Str, Source>` | `{}` | Sources. Block label: the source name, `main` for the primary one. Written as `source` blocks. |
| `vendors` | `Map<VendorKind, Vendor>` | `{}` | Vendored dependencies. Block label: the vendoring tool. Written as `vendor` blocks. |
| `patches` | `List<Path>` | `[]` | Patches applied in order in $SRC after extraction. |
| `patch_strip` | `Int where 0..=10` | `1` | The -p value for patch. |
| `build_deps` | `Set<Pkg>` | `[]` | Tools that run during the build, always the dynamic variant. |
| `link_deps` | `Set<Pkg>` | `[]` | Libraries the package compiles and links against. |
| `check_deps` | `Set<Pkg>` | `[]` | Packages present only when the check phase runs. |
| `deps` | `Set<Pkg>` | `[]` | Runtime dependencies in addition to the detected ones. |
| `provides` | `Set<Str where matches(r"^virtual:[a-z0-9][a-z0-9._-]*$")>` | `[]` | Extra virtual capabilities. |
| `conflicts` | `Set<Pkg>` | `[]` | Packages that cannot be in the same view. |
| `replaces` | `Set<Pkg>` | `[]` | Package names this package supersedes. |
| `ignore_deps` | `List<Str>` | `[]` | Globs of payload paths excluded from dependency detection. |
| `build` | `Build` |  | The build style. |
| `shell` | `Shell` | `sh` | The shell of the phase scripts: POSIX `sh`, or `bash` for a build that needs it, which adds `bash` to `build_deps`. Say why in a comment. The `kernel` style always uses `bash`. |
| `phases` | `Phases` | `Phases {}` | Phase replacements, hooks and the start directory. |
| `env` | `Map<Str, Str>` | `{}` | Extra environment variables for every phase. |
| `cflags` | `List<Str>` | `[]` | Appended to the default CFLAGS. |
| `cxxflags` | `List<Str>` | `[]` | Appended to the default CXXFLAGS. |
| `ldflags` | `List<Str>` | `[]` | Appended to the default LDFLAGS. |
| `rustflags` | `List<Str>` | `[]` | Appended to the default RUSTFLAGS. |
| `goflags` | `List<Str>` | `[]` | Appended to the default GOFLAGS. |
| `hardening_disable` | `Set<Hardening>` | `[]` | Default hardening to turn off. |
| `lto` | `Bool` | `false` | Link time optimization. |
| `check` | `Bool` | `true` | Run the check phase. |
| `parallel` | `Bool` | `true` | Build with more than one job. |
| `strip` | `Bool` | `true` | Strip ELF files. |
| `debug_split` | `Bool` | `true` | Split debug info into the -dbg subpackage. |
| `static_pie` | `Bool` | `true` | Static variant only: link executables with -static-pie. |
| `keep_archives` | `Bool` | `false` | Dynamic variant only: keep usr/lib/*.a. |
| `keep_dirs` | `List<Str>` | `[]` | Empty directories to keep in the payload. |
| `abi_epoch` | `Int where 0..=1000000` | `0` | The ABI generation of this package's libraries. It never decreases. |
| `split` | `Set<Split>` | `[dev, doc, dbg]` | Automatic subpackages to create. |
| `subpackages` | `Map<Str, Subpackage>` | `{}` | Explicit subpackages. Block label: the suffix. Written as `subpackage` blocks. |
| `alternatives` | `Map<Str, Alternative>` | `{}` | Alternative groups. Block label: the group name. Written as `alternative` blocks. |
| `setuid` | `Set<Str>` | `[]` | Payload paths that request a setuid or setgid bit. |
| `caps` | `Map<Str, Str>` | `{}` | Payload paths with requested file capabilities. |
| `ownership` | `Map<Str, Str>` | `{}` | Payload globs with their "user:group" owner. |
| `smoke` | `Option<Str>` |  | A script run in a view with only this package and its runtime dependencies. |
| `update` | `Option<Update>` |  | How new upstream versions are found. |
| `cpe` | `List<Str>` | `[]` | CPE 2.3 product prefixes for CVE matching. |
| `ci` | `Ci` | `Ci {}` | The CI runner and time limit. |

### Build styles

The `build` field takes one of these variants, written as a block whose label is the style and whose body sets its arguments, such as `build configure { args = ["--disable-nls"] }`.

| Style | Arguments |
|---|---|
| `custom` | none: every phase comes from `phases` |
| `configure` | `args = []`, `standard_args = true`, `autoreconf = false`, `out_of_tree = false`, `make_args = []`, `check_target = "check"`, `install_args = []` |
| `make` | `make_args = []`, `check_target = "check"`, `install_args = []` |
| `meson` | `options = []`, `auto_features = disabled` |
| `cmake` | `defines = []`, `generator = ninja` |
| `cargo` | `features = []`, `default_features = true`, `package`, `bins`, `args = []` |
| `go` | `packages = ["."]`, `ldflags = []`, `tags = []`, `cgo = false` |
| `python-pep517` | `check_cmd` |
| `kernel` | `config` (a path), `localversion`, `flavor`, `make_args = []` |

### Value types

**`Variant`**: A build configuration of a package. `dynamic | static`

**`Libc`**: A C library. `glibc | musl`

**`Split`**: An automatic subpackage. `dev | doc | dbg | locale`

**`VendorKind`**: A tool that vendors dependencies. `cargo | go`

**`Hardening`**: A default hardening flag that a recipe can turn off. `fortify | pie | relro | now | stack-clash | cf-protection | format | plt`

**`Runner`**: The class of machine a package builds on. `standard | big`

**`Feature`**: The meson `auto_features` setting. `enabled | disabled | auto`

**`Generator`**: The build tool cmake generates files for. `ninja | make`

**`Shell`**: The shell that runs the phase scripts. `sh | bash`

### `Source`

A source of a package. Exactly one of `url` and `git` is set.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `url` | `Option<List<Str>>` |  | Download locations, tried in order. https:// only. |
| `git` | `Option<Str where matches(r"^https://.*")>` |  | A clone URL. https:// only. |
| `commit` | `Option<Str where matches(r"^[0-9a-f]{40}$")>` |  | The full commit hash of a git source. |
| `submodules` | `Bool` | `false` | Fetch git submodules at their recorded commits. |
| `sha256` | `Option<Str where matches(r"^sha256:[0-9a-f]{64}$")>` |  | The SHA-256 hash of the file, or of the tarball made from a git checkout. |
| `file` | `Option<Str>` |  | The name of the downloaded file. none: the last URL path segment. |
| `extract` | `Option<Bool>` |  | Whether to extract the file. none: by its extension. |
| `strip_components` | `Option<Int where 0..=16>` |  | Leading path components to drop. none: one when the archive holds a single directory. |
| `dest` | `Option<Str>` |  | The directory under $SRC. none: "" for main, the label otherwise. |

### `Vendor`

Vendored dependencies, produced by running the vendoring tool on a source.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `sha256` | `Str where matches(r"^sha256:[0-9a-f]{64}$")` |  | The hash of the normalized vendor tarball. |
| `dir` | `Str` | `""` | The directory under $SRC holding Cargo.lock or go.mod. |
| `source` | `Str` | `"main"` | The source the lock file comes from. |

### `Phases`

Phase replacements, hooks and the start directory.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `subdir` | `Option<Str>` |  | The directory under $SRC every phase starts in. |
| `prepare` | `Option<Str>` |  | Replaces the prepare script. |
| `configure` | `Option<Str>` |  | Replaces the configure script. |
| `build` | `Option<Str>` |  | Replaces the build script. |
| `check` | `Option<Str>` |  | Replaces the check script. |
| `install` | `Option<Str>` |  | Replaces the install script. |
| `post_install` | `Option<Str>` |  | Runs as its own phase after install. |
| `pre_configure` | `Option<Str>` |  | Runs before the configure script. |
| `post_configure` | `Option<Str>` |  | Runs after the configure script. |
| `pre_build` | `Option<Str>` |  | Runs before the build script. |
| `post_build` | `Option<Str>` |  | Runs after the build script. |
| `pre_check` | `Option<Str>` |  | Runs before the check script. |
| `post_check` | `Option<Str>` |  | Runs after the check script. |
| `pre_install` | `Option<Str>` |  | Runs before the install script. |

### `Subpackage`

An explicit subpackage. Block label: the suffix.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `summary` | `Option<Str>` |  | none: the package summary followed by the suffix in parentheses. |
| `files` | `List<Str> where nonEmpty` |  | Globs of the payload paths it contains. |
| `exclude` | `List<Str>` | `[]` | Globs removed from the match. |
| `deps` | `Set<Pkg>` | `[]` | Runtime dependencies of this subpackage. |
| `provides` | `Set<Str>` | `[]` | Capabilities of this subpackage. |
| `conflicts` | `Set<Pkg>` | `[]` | Conflicts of this subpackage. |
| `replaces` | `Set<Pkg>` | `[]` | Package names this subpackage supersedes. |
| `depends_on_main` | `Bool` | `false` | Add an exact dependency on the main package. |
| `license` | `Option<List<Spdx>>` |  | none: the package's licenses. |
| `keep_empty` | `Bool` | `false` | Emit the subpackage even when no file matched. |

### `Alternative`

Membership in an alternative group. Block label: the group name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `paths` | `List<Str> where nonEmpty` |  | Payload paths the package ships for this group. |
| `priority` | `Int where -1000000..=1000000` | `0` | Higher wins when neither the configuration nor the repository default chooses. |
| `subpackage` | `Option<Str>` |  | The subpackage that contains the paths. none: the main package. |

### `Ci`

The CI runner and time limit.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `runner` | `Runner` | `standard` | The class of machine. |
| `timeout_minutes` | `Int where 1..=1440` | `360` | The longest a build may take. |

### `Update`

How new upstream versions are found.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `github` | `Option<Str where matches(r"^[^/]+/[^/]+$")>` |  | "owner/repo" on GitHub. |
| `gitlab` | `Option<Str>` |  | "host/group/project" on a GitLab instance. |
| `anitya` | `Option<Int where 1..=100000000>` |  | A project id on release-monitoring.org. |
| `url` | `Option<Str where matches(r"^https://.*")>` |  | A page to fetch. Requires `pattern`. |
| `pattern` | `Option<Str>` |  | A regular expression with one capture group for the version. |
| `tag_prefix` | `Option<Str>` |  | A prefix stripped from tags. |
| `track` | `Option<Str>` |  | Only versions with this prefix. |
| `ignore` | `List<Str>` | `["alpha", "beta", "rc", "pre", "dev", "snapshot"]` | Candidates containing any of these are dropped. |
| `prerelease` | `Bool` | `false` | Keep prereleases. |
| `automerge` | `Bool` | `false` | Bump changes may merge automatically. |
| `disabled` | `Option<Str>` |  | Why the package is never reported as outdated. |

## The repository descriptor

`recipes.stav` at the top of a recipe repository holds exactly one `recipe_repo` block, labeled with the repository name.

### `RecipeRepo`

The identity of a recipe repository.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `arch` | `Set<Str> where nonEmpty` | `["x86_64-v3"]` | Architectures this repository builds for. |
| `extends` | `List<Str>` | `[]` | Recipe repositories whose recipes this one overrides or depends on. |
| `toolchain_epoch` | `Int where 0..=1000000` | `0` | Incrementing it forces a rebuild of every package in the repository. |
| `source_mirrors` | `List<Str>` | `[]` | Content addressed source mirrors, tried after a source's own URLs. |

## Stage files

A stage file holds one `stage` block, which [`laft-build stage`](./laft-build#laft-build-stage-file) runs.

### `Stage`

A stage of a bootstrap: steps that build packages from one recipe repository into a root directory or the build-side store, or run scripts, in order.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `recipes` | `Str` |  | The recipe repository of the packages, relative to the stage file. |
| `install` | `Install` | `direct` | `direct` installs every build straight into the root and makes no package. `packages` makes Laft packages, keeps them in the output directory of the build root and unpacks them into the root, all but the debug info. `store` builds every package in a sandbox with a build view of the build-side store, which its packages enter for the builds after it. A `store` stage has no root. |
| `path` | `List<Str>` | `[]` | Directories below the root that steps on the host search for programs before the host's own, such as the bin directory of a cross toolchain. |
| `steps` | `List<Step> where nonEmpty` |  | The steps, in the order they run. |

### `Step`

One step of a stage. Exactly one of `packages` and `script` is set.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | `Str where matches(r"^[a-z0-9][a-z0-9-]*$")` |  | The name in progress reports and logs, unique in the stage. |
| `place` | `Place` | `host` | On the host, inside the root entered as a chroot, or, for a script of a `store` stage, in a sandbox whose `/usr` is a view of the build-side store that holds the packages of `view`. |
| `packages` | `List<Pkg>` | `[]` | Packages built in this order and installed into the root or the store. |
| `script` | `Option<Str>` |  | A POSIX shell script, such as a checkpoint. It stops at the first command that fails, like a phase. It runs once, and again when it or an earlier step changes. `$ROOT` is the root as the script sees it, unset in a view. |
| `env` | `Map<Str, Str>` | `{}` | Extra environment variables of the script. |
| `view` | `List<Pkg>` | `[]` | The packages of the view a script with `place = view` runs in. Their runtime dependencies come with them. |

**`Place`**: Where a step of a stage runs. `host | root | view`

**`Install`**: How the packages of a stage reach its root. `direct | packages | store`
