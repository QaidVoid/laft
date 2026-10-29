use super::*;

fn input(variant: Variant, hardening_disable: &[Hardening]) -> EnvInput<'_> {
    EnvInput {
        name: "foo",
        version: "1.0",
        rev: 1,
        target: Target::new(variant),
        repo: "core",
        hardening_disable,
        lto: false,
        static_pie: true,
        cflags: &[],
        cxxflags: &[],
        ldflags: &[],
        rustflags: &[],
        goflags: &[],
        cgo: false,
    }
}

fn words(env: &BTreeMap<String, String>, key: &str) -> Vec<String> {
    env[key].split(' ').map(String::from).collect()
}

#[test]
fn disabled_hardening_removes_exactly_its_flags() {
    let all = environment(&input(Variant::Dynamic, &[]));
    let removed = |h: Hardening, key: &str| {
        let env = environment(&input(Variant::Dynamic, &[h]));
        let now = words(&env, key);
        words(&all, key)
            .into_iter()
            .filter(|w| !now.contains(w))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        removed(Hardening::Fortify, "CFLAGS"),
        ["-Wp,-D_FORTIFY_SOURCE=3"]
    );
    assert_eq!(removed(Hardening::Relro, "LDFLAGS"), ["-Wl,-z,relro"]);
    assert_eq!(removed(Hardening::Now, "LDFLAGS"), ["-Wl,-z,now"]);
    assert_eq!(
        removed(Hardening::Relro, "RUSTFLAGS"),
        ["link-arg=-Wl,-z,relro"]
    );
    assert_eq!(
        removed(Hardening::Now, "RUSTFLAGS"),
        ["link-arg=-Wl,-z,now"]
    );
    assert_eq!(
        removed(Hardening::StackClash, "CXXFLAGS"),
        ["-fstack-clash-protection"]
    );
    assert_eq!(
        removed(Hardening::CfProtection, "CFLAGS"),
        ["-fcf-protection"]
    );
    assert_eq!(
        removed(Hardening::Format, "CFLAGS"),
        ["-Wformat", "-Werror=format-security"]
    );
    assert_eq!(removed(Hardening::Plt, "CFLAGS"), ["-fno-plt"]);

    let pie = environment(&input(Variant::Dynamic, &[Hardening::Pie]));
    assert!(pie["CFLAGS"].ends_with(" -fno-pie"));
    assert!(pie["CXXFLAGS"].contains(" -fno-pie"));
    assert!(pie["LDFLAGS"].ends_with(" -no-pie"));
    let pie = environment(&input(Variant::Static, &[Hardening::Pie]));
    assert!(pie["LDFLAGS"].starts_with("-static -Wl,-O1"));
    assert!(pie["CFLAGS"].ends_with(" -fno-pie"));
}

#[test]
fn recipe_flags_follow_the_defaults_and_lto_adds_its_own() {
    let cflags = ["-O3".to_owned()];
    let rustflags = ["-C opt-level=3".to_owned()];
    let env = environment(&EnvInput {
        lto: true,
        cflags: &cflags,
        rustflags: &rustflags,
        ..input(Variant::Dynamic, &[])
    });
    assert!(
        env["CFLAGS"].ends_with(" -flto=auto -O3"),
        "{}",
        env["CFLAGS"]
    );
    assert!(env["CXXFLAGS"].contains(" -flto=auto"));
    assert!(env["RUSTFLAGS"].ends_with(" -C lto=thin -C opt-level=3"));
}

#[test]
fn static_builds_link_static_executables_with_the_musl_tools() {
    let env = environment(&EnvInput {
        static_pie: false,
        ..input(Variant::Static, &[])
    });
    assert!(env["LDFLAGS"].starts_with("-static -Wl,-O1"));
    assert!(!env["CFLAGS"].contains("FORTIFY"));
    assert!(!env["CFLAGS"].contains("-fno-plt"));
    assert_eq!(env["CC"], "x86_64-laft-linux-musl-gcc");
    assert_eq!(env["SYSROOT"], "/usr/x86_64-laft-linux-musl");
    let dynamic = environment(&input(Variant::Dynamic, &[]));
    assert!(!dynamic.contains_key("SYSROOT"));
    assert!(!dynamic.contains_key("PKG_CONFIG_LIBDIR"));
}

#[test]
fn cgo_passes_the_c_flags_to_go() {
    let plain = environment(&input(Variant::Dynamic, &[]));
    assert_eq!(plain["CGO_ENABLED"], "0");
    assert!(!plain.contains_key("CGO_CFLAGS"));
    let cgo = environment(&EnvInput {
        cgo: true,
        ..input(Variant::Dynamic, &[])
    });
    assert_eq!(cgo["CGO_ENABLED"], "1");
    assert_eq!(cgo["CGO_CFLAGS"], cgo["CFLAGS"]);
    assert_eq!(cgo["CGO_LDFLAGS"], cgo["LDFLAGS"]);
}

#[test]
fn every_variable_the_builder_sets_is_reserved() {
    for variant in [Variant::Dynamic, Variant::Static] {
        let env = environment(&EnvInput {
            cgo: true,
            ..input(variant, &[])
        });
        for key in env.keys() {
            assert!(RESERVED.contains(&key.as_str()), "{key}");
        }
    }
}

#[test]
fn an_install_environment_names_no_toolchain() {
    for variant in [Variant::Dynamic, Variant::Static] {
        let env = install(environment(&input(variant, &[])));
        for key in [
            "CC",
            "CXX",
            "LD",
            "AR",
            "CFLAGS",
            "LDFLAGS",
            "CHOST",
            "PKG_CONFIG",
        ] {
            assert!(!env.contains_key(key), "{variant}: {key} is set");
        }
        assert_eq!(env["DESTDIR"], "/out");
        assert_eq!(env["SRC"], "/build/src");
        assert_eq!(env["SOURCE_DATE_EPOCH"], SOURCE_DATE_EPOCH.to_string());
    }
}

#[test]
fn every_link_asks_for_a_build_id_that_debug_info_is_split_by() {
    for variant in [Variant::Dynamic, Variant::Static] {
        let env = environment(&input(variant, &[]));
        assert!(
            env["LDFLAGS"].contains("-Wl,--build-id=sha1"),
            "{variant}: {}",
            env["LDFLAGS"]
        );
        assert!(
            env["RUSTFLAGS"].contains("-C link-arg=-Wl,--build-id=sha1"),
            "{variant}: {}",
            env["RUSTFLAGS"]
        );
    }
}
