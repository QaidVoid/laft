# Laft agent rules

Laft is a Linux distribution and the Rust toolchain that builds and runs it. Packages live in a content addressed store. Every `/usr` a process sees is a view assembled from that store with hardlinks. System states are immutable, numbered generations that switch atomically. The system is described by a declarative configuration written in Stav, and services are declared in an init-agnostic model rendered for runit. These rules apply to every change in this repository.

## System invariants

These properties define Laft. A change that breaks one is a bug, even when every test passes.

- Store entries are immutable once published. Nothing writes into a store entry after the rename that publishes it.
- `/laft` is mounted read-only, `nosuid` and `nodev` in the initial mount namespace. Laft writes to it only from a private mount namespace.
- Store files never carry setuid, setgid or file capabilities. Only the mounted generations honor privileged copies, and old generations and non-system views never do.
- `/usr` always resolves to a complete generation. Switching generations is a single `rename(2)`.
- No package runs code on the client. Side effects come only from triggers that Laft itself implements.
- Two packages never own the same path in one view unless the configuration chooses one.
- Every repository index is signed and carries a generation number that never decreases. Clients refuse anything else.
- Losing power at any moment of a transaction leaves a system that boots into either the old or the new generation.
- Every Laft binary is statically linked and runs with an empty `/usr`.
- Package identity is read from manifests, never parsed from paths.

## Workspace

- Crates live in `crates/<name>/`. Each crate has one clear responsibility. A new crate is justified only by a heavy dependency or a real boundary, not by tidiness. Create a crate when its first real code lands, not before.
- `laft-core` holds the identifiers and formats shared by every component. It depends on no other workspace crate.
- The setuid helper `laft-enter` depends only on `laft-core` and `laft-sandbox`, and its own code stays small enough to audit in one sitting. The initramfs init `laft-boot` depends only on `laft-core` and crates.io crates.
- Client crates never depend on build-side crates.
- Every binary builds for `x86_64-unknown-linux-musl` as a static executable with mimalloc as the global allocator.
- Laft uses Stav only through releases of the `stav` crate on crates.io. Never patch it, vendor it or point a path dependency at a local checkout. When Laft needs a language change, make it in the Stav repository, release it, then update the dependency here.
- Every crate manifest inherits the workspace package fields and lints, and has a one-line `description`, which crates.io requires.
- Everything targets Rust edition 2024 on the stable toolchain pinned in `rust-toolchain.toml`.

## Version control

- Check which VCS the checkout uses before the first VCS command: run `jj root`. If it succeeds, use `jj` for everything and never run `git` directly, because git commands can desync a colocated jj repository. If it fails, the checkout is plain git, so use `git` for everything.
- Never push and never open pull requests unless the user explicitly asks.
- Commit messages use the semantic format `type: imperative message` or `type(scope): imperative message`, for example `feat(store): publish entries with an atomic rename`. Keep the first line within 68 characters. Keep bodies short.
- Commit a change once it is a coherent unit and all validation gates pass. Do not commit work that fails a gate.
- Do not hold changes too long either. Do not let more than one task's worth of work pile up uncommitted. Split large tasks into several validated commits.
- With jj, use `jj describe -m "<message>"` to describe the working-copy change and `jj new` to start the next one. Run `jj status` and `jj diff` before describing, so the commit contains only what you intend.
- With git, stage only the files you intend with `git add <paths>`, never `git add -A` or `git add .`, then run `git commit -m "<message>"`. Run `git status` and `git diff --staged` before committing. Never amend, rebase or force anything unless the user asks.

## Releases

- Releases go through release-plz only (`.release-plz.toml` and `.github/workflows/release-plz.yml`). On every push to `main`, it opens or updates a release PR with version bumps and changelogs, and publishes to crates.io when that PR is merged.
- Never run `cargo publish`, never bump versions by hand, and never edit `CHANGELOG.md` files by hand. release-plz derives all of them from the semantic commit messages, so write those messages with care. `feat` and `fix` commits appear in the changelog, and a `!` or a `BREAKING CHANGE:` footer marks a breaking change.
- `semver_check` is on. A breaking change to an API that has been released must be marked as breaking in its commit message. A change to an API that no release has shipped yet is not breaking, so do not mark it.
- On-disk and on-wire formats (package files, manifests, indexes, generation records, state files) are versioned. Changing one requires a new format version and a reader for the old one, and the commit is marked as breaking.
- Crates that are not meant for crates.io set `publish = false`, and release-plz skips them.
- When a new publishable crate is added, add it to `changelog_include` of the `laft` package in `.release-plz.toml`.

## Dependencies

- Only crates.io dependencies. No git dependencies, no path dependencies outside this workspace, and no `[patch]` sections. This holds for transitive dependencies too.
- Add, remove and change dependencies only with `cargo add` and `cargo remove`. Never hand-edit dependency tables in a manifest. Use `cargo add --no-default-features --features ...` to keep the feature set minimal.
- Every dependency must build for `x86_64-unknown-linux-musl` without system libraries, because every binary is static. Prefer pure Rust implementations.
- Keep the dependency trees of `laft-enter` and `laft-boot` as small as possible. Every crate in them runs as root before anything else can protect the system.
- Vet every dependency before adding it, and state the result in the commit body. Check that it:
  - is needed, because the standard library or an existing dependency cannot do the job in a few lines,
  - has a permissive license (MIT, Apache-2.0, BSD, Zlib, ISC or Unicode),
  - is maintained, with a release in the last 12 months or a stable, finished scope,
  - pulls no git sources and downloads nothing at build time (inspect `build.rs` if one exists),
  - fits the MSRV,
  - adds no duplicate major versions (`cargo tree -d`) unless there is no alternative.

## Code

- Keep it simple. Do not add abstractions, generics, traits, feature flags or configuration for cases nothing uses yet. If a change seems to need that, stop and ask first.
- Optimize only with a measurement in hand. When complexity is added for performance, say so in a doc comment or commit body together with the measurement that justified it.
- Library code must not panic on bad input. Use `expect` only for internal invariants, and write the message as the invariant that was broken. Document every possible panic under a `# Panics` heading.
- `unsafe` is forbidden (`#![forbid(unsafe_code)]`) in every crate except `laft-sandbox` and `laft-elf`. There, every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound. Prefer `rustix` safe wrappers so that `unsafe` is rarely needed at all.
- Filesystem changes that must survive a crash are written to a temporary file in the same directory, `fsync`ed, then renamed into place, and the directory is `fsync`ed. Never rewrite a file in place.
- Anything hashed or signed is serialized as canonical JSON. Never hash a pretty-printed or ad hoc serialization.
- Use ordered collections wherever iteration order can reach a hash, a file, a manifest or user-visible output.

## Comments and documentation

- Every public item has a doc comment. That includes modules, structs, enums, variants, fields, traits, functions, methods, constants and type aliases. Say what it is for and how it behaves. Cover `# Errors` and `# Panics` where they apply, and give examples for entry-point APIs.
- No narration and no obvious comments. Never restate what the code does. Write an inline comment only when the code cannot show why something is done: a non-obvious invariant, a kernel or filesystem quirk or a deliberate trade-off.
- Never reference other projects, distributions, frameworks or applications in code, comments, docs or commit messages, except the programs Laft actually runs or packages. Describe the behavior itself instead of what inspired it.
- Never reference design documents, specifications, planning notes, decision records, section numbers, milestones or task IDs in code, comments, docs, tests or commit messages. The code and its documentation must stand on their own.
- Public standards and formats that the code implements may be named, for example an RFC, POSIX, ELF or the JSON and TOML formats, because naming them tells the reader exactly what behavior to expect.
- Do not use em-dashes in any prose, and do not substitute them with `--`, a spaced hyphen or a semicolon. Write complete sentences.

## Tests

- Unit tests live in a sibling `<module>_tests.rs` file, never inline in the implementation file. Wire it at the bottom of the module:

  ```rust
  #[cfg(test)]
  #[path = "version_tests.rs"]
  mod tests;
  ```

  For `foo/mod.rs` the file is `foo/mod_tests.rs`.
- Integration tests that exercise a crate through its public API go in `crates/<name>/tests/`. End-to-end tests that boot a machine live in `tests/vm/` and are not part of the gates.
- Test behavior, not implementation. A test must fail if a user-visible behavior or a documented invariant breaks. Do not write tests for getters, derives, constructors or anything the compiler already guarantees.
- Name each test after the behavior it checks, for example `lower_index_generation_is_rejected`.
- Use property tests (`proptest`) for invariants over many inputs, such as version ordering being a total order, canonical JSON round trips and resolver determinism. Fix seeds when a failure is found and keep the regression file.
- Snapshot tests use `insta`. Review every snapshot change on purpose with `cargo insta review` or by reading the diff. Never accept snapshots blindly.
- Crash safety is tested by interrupting a transaction at every recorded step and checking that recovery leaves a consistent tree.
- Tests never touch the host system. They work only in temporary directories, loop images or virtual machines. Never run `mkfs`, partitioning tools, `mount` on host paths, or anything that writes to the real `/laft`, `/etc`, `/boot` or `/usr`.
- Tests that need root, namespaces, loop devices or a virtual machine must skip cleanly, with a printed reason, when the capability is missing. They must never fail for that reason.
- Fuzz targets live in `fuzz/` (cargo-fuzz, nightly) and are not part of the workspace or the gates. Every parser of untrusted input has one: package files, manifests, indexes, signatures and ELF files. When you change such a parser, run its fuzz target for at least five minutes before committing.
- Benchmarks are not tests and do not run in the gates.

## Validation gates

Run these from the workspace root before claiming any work is done and before every commit:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
! grep -q 'source = "git+' Cargo.lock
```

Once the workspace contains a binary, also build every binary statically and check that it has no dynamic loader:

```sh
cargo build --workspace --release --target x86_64-unknown-linux-musl
```

- Never claim work is done, fixed or passing without running the gates and reading their output. If a gate fails, fix it or report the failure exactly as it appeared. Never silence a lint with `allow` to pass a gate unless the lint is wrong for that spot, and then write the reason next to the `allow`.
- Verify behavior, not only compilation. Run the relevant test or command and check the result.
