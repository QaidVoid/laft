//! The environment of build phases and the default compiler flags.

use std::collections::BTreeMap;

use laft_core::Variant;

use crate::Target;
use crate::schema::Hardening;
use crate::target::{CPU, GNU_TRIPLE, MUSL_TRIPLE, SYSROOT};

/// The number of parallel jobs, set by the builder.
pub(crate) const JOBS: &str = "JOBS";

/// `-j` with the number of parallel jobs, set by the builder.
pub(crate) const MAKEFLAGS: &str = "MAKEFLAGS";

/// `SOURCE_DATE_EPOCH`: 1980-01-01T00:00:00Z, the earliest time ZIP based
/// formats can represent.
pub const SOURCE_DATE_EPOCH: u64 = 315_532_800;

/// Every variable the builder defines. A recipe's `env` replaces them only
/// with `force(...)`.
pub(crate) const RESERVED: &[&str] = &[
    "AR",
    "AS",
    "BUILDDIR",
    "CARGO_HOME",
    "CARGO_OUT",
    "CARGO_PROFILE_RELEASE_DEBUG",
    "CARGO_TARGET",
    "CARGO_TARGET_DIR",
    "CBUILD",
    "CC",
    "CFLAGS",
    "CGO_CFLAGS",
    "CGO_CXXFLAGS",
    "CGO_ENABLED",
    "CGO_LDFLAGS",
    "CHOST",
    "COMMON",
    "CPP",
    "CPPFLAGS",
    "CXX",
    "CXXFLAGS",
    "DESTDIR",
    "GOAMD64",
    "GOCACHE",
    "GOFLAGS",
    "GOPATH",
    "GOPROXY",
    "GOTOOLCHAIN",
    "HOME",
    JOBS,
    "LAFT_ARCH",
    "LAFT_CROSS_DIR",
    "LAFT_NAME",
    "LAFT_REV",
    "LAFT_VARIANT",
    "LAFT_VERSION",
    "LANG",
    "LC_ALL",
    "LD",
    "LDFLAGS",
    MAKEFLAGS,
    "NM",
    "OBJCOPY",
    "OBJDUMP",
    "OUT",
    "PATH",
    "PKG_CONFIG",
    "PKG_CONFIG_LIBDIR",
    "PKG_CONFIG_SYSROOT_DIR",
    "PREFIX",
    "PYTHONHASHSEED",
    "RANLIB",
    "READELF",
    "RECIPE",
    "RUSTFLAGS",
    "SOURCE_DATE_EPOCH",
    "SRC",
    "SYSROOT",
    "TMPDIR",
    "TZ",
];

/// The variables that name a compiler, the binutils, pkg-config, a
/// sysroot or the flags of Laft's toolchain.
const TOOLCHAIN: &[&str] = &[
    "AR",
    "AS",
    "CBUILD",
    "CC",
    "CFLAGS",
    "CGO_CFLAGS",
    "CGO_CXXFLAGS",
    "CGO_LDFLAGS",
    "CHOST",
    "CPP",
    "CPPFLAGS",
    "CXX",
    "CXXFLAGS",
    "LD",
    "LDFLAGS",
    "NM",
    "OBJCOPY",
    "OBJDUMP",
    "PKG_CONFIG",
    "PKG_CONFIG_LIBDIR",
    "PKG_CONFIG_SYSROOT_DIR",
    "RANLIB",
    "READELF",
    "RUSTFLAGS",
    "SYSROOT",
];

/// The binutils tools, by variable name.
const BINUTILS: &[(&str, &str)] = &[
    ("AR", "ar"),
    ("AS", "as"),
    ("LD", "ld"),
    ("NM", "nm"),
    ("OBJCOPY", "objcopy"),
    ("OBJDUMP", "objdump"),
    ("RANLIB", "ranlib"),
    ("READELF", "readelf"),
];

const FORTIFY: &str = "-Wp,-D_FORTIFY_SOURCE=3";

/// What the environment of one build depends on.
pub(crate) struct EnvInput<'a> {
    pub(crate) name: &'a str,
    pub(crate) version: &'a str,
    pub(crate) rev: u32,
    pub(crate) target: Target,
    /// The repository that defines the package.
    pub(crate) repo: &'a str,
    pub(crate) hardening_disable: &'a [Hardening],
    pub(crate) lto: bool,
    pub(crate) static_pie: bool,
    pub(crate) cflags: &'a [String],
    pub(crate) cxxflags: &'a [String],
    pub(crate) ldflags: &'a [String],
    pub(crate) rustflags: &'a [String],
    pub(crate) goflags: &'a [String],
    /// Whether the `go` style enables cgo.
    pub(crate) cgo: bool,
}

/// The compiler and linker flags of one build.
struct Flags {
    cflags: Vec<String>,
    cxxflags: Vec<String>,
    ldflags: Vec<String>,
}

/// Returns the default flags for `input`, with hardening removed as the
/// recipe asks and the recipe's own flags appended.
fn flags(input: &EnvInput<'_>) -> Flags {
    let dynamic = input.target.variant() == Variant::Dynamic;
    let off = |h| input.hardening_disable.contains(&h);
    let prefix_map = |dir: &str| format!("-ffile-prefix-map={dir}=/usr/src/debug/{}", input.name);
    let mut cflags: Vec<String> = vec![format!("-march={CPU}"), "-mtune=generic".into()];
    cflags.extend(["-O2", "-pipe"].map(String::from));
    if dynamic && !off(Hardening::Plt) {
        cflags.push("-fno-plt".into());
    }
    cflags.push("-fexceptions".into());
    if dynamic && !off(Hardening::Fortify) {
        cflags.push(FORTIFY.into());
    }
    if !off(Hardening::Format) {
        cflags.extend(["-Wformat", "-Werror=format-security"].map(String::from));
    }
    if !off(Hardening::StackClash) {
        cflags.push("-fstack-clash-protection".into());
    }
    if !off(Hardening::CfProtection) {
        cflags.push("-fcf-protection".into());
    }
    cflags.extend(
        [
            "-fno-omit-frame-pointer",
            "-mno-omit-leaf-frame-pointer",
            "-g",
        ]
        .map(String::from),
    );
    cflags.extend([prefix_map("/build/src"), prefix_map("/build/obj")]);
    if off(Hardening::Pie) {
        cflags.push("-fno-pie".into());
    }
    if input.lto {
        cflags.push("-flto=auto".into());
    }

    let mut cxxflags = cflags.clone();
    if dynamic {
        cxxflags.push("-Wp,-D_GLIBCXX_ASSERTIONS".into());
    }

    let mut ldflags = Vec::new();
    if !dynamic {
        let pie = input.static_pie && !off(Hardening::Pie);
        ldflags.push(if pie { "-static-pie" } else { "-static" }.into());
    }
    ldflags.extend(["-Wl,-O1", "-Wl,--sort-common", "-Wl,--as-needed"].map(String::from));
    if !off(Hardening::Relro) {
        ldflags.push("-Wl,-z,relro".into());
    }
    if !off(Hardening::Now) {
        ldflags.push("-Wl,-z,now".into());
    }
    if dynamic {
        ldflags.push("-Wl,-z,pack-relative-relocs".into());
        if off(Hardening::Pie) {
            ldflags.push("-no-pie".into());
        }
    }

    cflags.extend(input.cflags.iter().cloned());
    cxxflags.extend(input.cxxflags.iter().cloned());
    ldflags.extend(input.ldflags.iter().cloned());
    Flags {
        cflags,
        cxxflags,
        ldflags,
    }
}

/// Returns the environment of every phase, without `JOBS` and
/// `MAKEFLAGS`.
pub(crate) fn environment(input: &EnvInput<'_>) -> BTreeMap<String, String> {
    let variant = input.target.variant();
    let dynamic = variant == Variant::Dynamic;
    let name = input.name;
    let mut env = BTreeMap::new();
    let mut set = |key: &str, value: String| {
        env.insert(key.to_owned(), value);
    };

    set("PATH", "/usr/bin".into());
    set("HOME", "/build/home".into());
    set("TMPDIR", "/build/tmp".into());
    set("LANG", "C.UTF-8".into());
    set("LC_ALL", "C.UTF-8".into());
    set("TZ", "UTC".into());
    set("SOURCE_DATE_EPOCH", SOURCE_DATE_EPOCH.to_string());
    set("SRC", "/build/src".into());
    set("BUILDDIR", "/build/obj".into());
    set("OUT", "/out".into());
    set(
        "DESTDIR",
        if dynamic {
            "/out".into()
        } else {
            format!("/out{SYSROOT}")
        },
    );
    set("RECIPE", format!("/build/recipes/{}/{name}", input.repo));
    set("COMMON", format!("/build/common/{}", input.repo));
    set("PREFIX", "/usr".into());
    set("CBUILD", GNU_TRIPLE.into());
    set("CHOST", input.target.triple().into());
    set("LAFT_CROSS_DIR", "/build/cross".into());
    set("PYTHONHASHSEED", "0".into());
    set("LAFT_NAME", name.into());
    set("LAFT_VERSION", input.version.into());
    set("LAFT_REV", input.rev.to_string());
    set("LAFT_VARIANT", variant.to_string());
    set("LAFT_ARCH", crate::target::ARCH.into());

    let tool = |tool: &str| {
        if dynamic {
            tool.to_owned()
        } else {
            format!("{MUSL_TRIPLE}-{tool}")
        }
    };
    set("CC", tool("gcc"));
    set("CXX", tool("g++"));
    set("CPP", tool("cpp"));
    for (key, binary) in BINUTILS {
        set(key, tool(binary));
    }
    if dynamic {
        set("PKG_CONFIG", "pkgconf".into());
    } else {
        set("SYSROOT", SYSROOT.into());
        set("PKG_CONFIG", "pkgconf --static".into());
        set("PKG_CONFIG_SYSROOT_DIR", SYSROOT.into());
        set(
            "PKG_CONFIG_LIBDIR",
            format!("{SYSROOT}/usr/lib/pkgconfig:{SYSROOT}/usr/share/pkgconfig"),
        );
    }

    let flags = flags(input);
    let cflags = flags.cflags.join(" ");
    let cxxflags = flags.cxxflags.join(" ");
    let ldflags = flags.ldflags.join(" ");
    set("CPPFLAGS", String::new());
    if input.cgo {
        set("CGO_CFLAGS", cflags.clone());
        set("CGO_CXXFLAGS", cxxflags.clone());
        set("CGO_LDFLAGS", ldflags.clone());
    }
    set("CFLAGS", cflags);
    set("CXXFLAGS", cxxflags);
    set("LDFLAGS", ldflags);

    let mut rustflags: Vec<String> = [
        format!("-C target-cpu={CPU}"),
        "-C debuginfo=2".into(),
        "-C strip=none".into(),
        format!("--remap-path-prefix=/build=/usr/src/debug/{name}"),
    ]
    .into();
    if !input.hardening_disable.contains(&Hardening::Relro) {
        rustflags.push("-C link-arg=-Wl,-z,relro".into());
    }
    if !input.hardening_disable.contains(&Hardening::Now) {
        rustflags.push("-C link-arg=-Wl,-z,now".into());
    }
    if !dynamic {
        rustflags.extend([
            "-C target-feature=+crt-static".into(),
            "-C link-self-contained=no".into(),
            format!("-C linker={MUSL_TRIPLE}-gcc"),
        ]);
    }
    if input.lto {
        rustflags.push("-C lto=thin".into());
    }
    rustflags.extend(input.rustflags.iter().cloned());
    set("RUSTFLAGS", rustflags.join(" "));
    set("CARGO_HOME", "/build/cargo-home".into());
    set("CARGO_TARGET_DIR", "/build/obj/target".into());
    set(
        "CARGO_TARGET",
        if dynamic {
            "x86_64-unknown-linux-gnu"
        } else {
            "x86_64-unknown-linux-musl"
        }
        .into(),
    );
    set(
        "CARGO_OUT",
        format!(
            "/build/obj/target/{}/release",
            if dynamic {
                "x86_64-unknown-linux-gnu"
            } else {
                "x86_64-unknown-linux-musl"
            }
        ),
    );
    set("CARGO_PROFILE_RELEASE_DEBUG", "2".into());

    let mut goflags: Vec<String> = ["-trimpath", "-mod=vendor", "-modcacherw", "-buildvcs=false"]
        .map(String::from)
        .into();
    goflags.extend(input.goflags.iter().cloned());
    set("GOFLAGS", goflags.join(" "));
    set("GOAMD64", "v3".into());
    set("GOCACHE", "/build/go-cache".into());
    set("GOPATH", "/build/go".into());
    set("GOPROXY", "off".into());
    set("GOTOOLCHAIN", "local".into());
    set("CGO_ENABLED", if input.cgo { "1" } else { "0" }.into());
    env
}

/// Returns the environment `env` of a build that installs into a system
/// under construction: without the [`TOOLCHAIN`] variables, even forced
/// ones, and with `DESTDIR` at `/out`. Such a system is built with tools
/// that are not Laft's toolchain, so its build systems choose their own
/// compilers and flags, for example from `--host`. The Go and Cargo
/// settings and `LAFT_CROSS_DIR` stay.
pub(crate) fn install(mut env: BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.retain(|key, _| !TOOLCHAIN.contains(&key.as_str()));
    env.insert("DESTDIR".to_owned(), "/out".to_owned());
    env
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
