use super::*;
use crate::schema::Phases as RecipePhases;

fn input(variant: Variant) -> StyleInput<'static> {
    StyleInput {
        name: "foo",
        rev: 3,
        variant,
        subdir: "",
        check: true,
        lto: false,
        kernel_config: Some("/build/recipes/core/foo/files/config"),
    }
}

fn no_phases() -> RecipePhases {
    RecipePhases {
        subdir: None,
        prepare: None,
        configure: None,
        build: None,
        check: None,
        install: None,
        post_install: None,
        pre_configure: None,
        post_configure: None,
        pre_build: None,
        post_build: None,
        pre_check: None,
        post_check: None,
        pre_install: None,
    }
}

#[test]
fn words_are_single_quoted_with_quotes_escaped() {
    assert_eq!(quote("it's"), r"'it'\''s'");
    assert_eq!(dq(r#"a"$b`\"#), r#"a\"\$b\`\\"#);
}

#[test]
fn hooks_run_around_the_style_script_in_one_shell() {
    let style = scripts(
        &Build::Make {
            make_args: vec!["V=1".into()],
            check_target: "test".into(),
            install_args: vec![],
        },
        &input(Variant::Dynamic),
    );
    let recipe = RecipePhases {
        pre_build: Some("echo before\n".into()),
        post_build: Some("echo after\n".into()),
        install: Some("make install-strip".into()),
        post_install: Some("rm -r \"$DESTDIR/usr/share/foo\"\n".into()),
        ..no_phases()
    };
    let phases = phases(style, &recipe, "src");
    let header = "#!/usr/bin/bash\nset -euo pipefail\numask 022\ncd \"$SRC/src\"\n";
    assert_eq!(
        phases.build.as_deref(),
        Some(format!("{header}echo before\nmake PREFIX=/usr 'V=1'\necho after\n").as_str())
    );
    assert_eq!(
        phases.check.as_deref(),
        Some(format!("{header}make PREFIX=/usr 'test'\n").as_str())
    );
    assert_eq!(
        phases.install.as_deref(),
        Some(format!("{header}make install-strip\n").as_str())
    );
    assert!(phases.post_install.is_some());
    assert_eq!(phases.prepare, None);
    assert_eq!(phases.configure, None);
}

#[test]
fn the_custom_style_runs_only_what_the_recipe_sets() {
    let recipe = RecipePhases {
        build: Some("make".into()),
        check: Some("  \n".into()),
        ..no_phases()
    };
    let phases = phases(
        scripts(&Build::Custom, &input(Variant::Dynamic)),
        &recipe,
        "",
    );
    let ran: Vec<&str> = phases
        .iter()
        .filter(|(_, script)| script.is_some())
        .map(|(name, _)| name)
        .collect();
    assert_eq!(ran, ["build"]);
}

#[test]
fn cmake_and_meson_cross_compile_static_builds() {
    let cmake = scripts(
        &Build::Cmake {
            defines: vec![],
            generator: Generator::Make,
        },
        &input(Variant::Static),
    );
    assert_eq!(
        cmake.configure.as_deref(),
        Some(
            "cmake -S \"$SRC/\" -B \"$BUILDDIR\" -G \"Unix Makefiles\" \\\n  \
             -DCMAKE_BUILD_TYPE=None -DCMAKE_INSTALL_PREFIX=/usr \\\n  \
             -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_INSTALL_SBINDIR=bin -DCMAKE_INSTALL_LIBEXECDIR=lib \\\n  \
             -DCMAKE_INSTALL_SYSCONFDIR=/etc -DBUILD_TESTING=true \\\n  \
             -DBUILD_SHARED_LIBS=OFF -DCMAKE_TOOLCHAIN_FILE=\"$LAFT_CROSS_DIR/toolchain.cmake\""
        )
    );
    let meson = scripts(
        &Build::Meson {
            options: vec![],
            auto_features: Feature::Auto,
        },
        &StyleInput {
            lto: true,
            ..input(Variant::Static)
        },
    );
    let configure = meson.configure.expect("meson configures");
    assert!(configure.contains("--auto-features=auto"));
    assert!(configure.contains("-Db_lto=true"));
    assert!(configure.ends_with(
        "--default-library=static --prefer-static --cross-file=\"$LAFT_CROSS_DIR/meson.ini\""
    ));
}

#[test]
fn configure_passes_the_standard_arguments_of_the_variant() {
    let build = Build::Configure {
        args: vec!["--with-x".into()],
        standard_args: true,
        autoreconf: true,
        out_of_tree: true,
        make_args: vec![],
        check_target: "check".into(),
        install_args: vec!["INSTALL=x".into()],
    };
    let dynamic = scripts(&build, &input(Variant::Dynamic));
    assert_eq!(
        dynamic.configure.as_deref(),
        Some(
            "if true; then cd \"$BUILDDIR\"; fi\n\"$SRC//configure\" --prefix=/usr --sysconfdir=/etc \
             --localstatedir=/var --bindir=/usr/bin --sbindir=/usr/bin --libdir=/usr/lib \
             --libexecdir=/usr/lib --mandir=/usr/share/man --infodir=/usr/share/info \
             --build=$CBUILD --host=$CHOST --disable-static --enable-shared '--with-x'"
        )
    );
    assert_eq!(
        dynamic.prepare.as_deref(),
        Some("if true; then autoreconf -fi; fi")
    );
    assert!(
        dynamic
            .install
            .as_deref()
            .is_some_and(|s| s.ends_with("make DESTDIR=\"$DESTDIR\" 'INSTALL=x' install"))
    );
    let configure = scripts(&build, &input(Variant::Static)).configure;
    assert!(configure.is_some_and(|s| s.contains("--enable-static --disable-shared '--with-x'")));
}

#[test]
fn cargo_selects_packages_and_features_in_build_and_check() {
    let cargo = scripts(
        &Build::Cargo {
            features: vec!["a".into(), "b".into()],
            default_features: false,
            package: Some("foo-cli".into()),
            bins: None,
            args: vec!["--locked".into()],
        },
        &input(Variant::Dynamic),
    );
    assert_eq!(
        cargo.build.as_deref(),
        Some(
            "cargo build --frozen --release --target \"$CARGO_TARGET\" \\\n  \
             -p 'foo-cli' --no-default-features \\\n  --features 'a,b' '--locked'"
        )
    );
    assert_eq!(
        cargo.check.as_deref(),
        Some(
            "cargo test --frozen --release --target \"$CARGO_TARGET\" -p 'foo-cli' \
             --no-default-features --features 'a,b'"
        )
    );
    assert!(
        cargo
            .install
            .is_some_and(|s| s.starts_with("for b in 'foo'; do"))
    );
}

#[test]
fn go_links_by_variant_and_cgo() {
    let go = |variant, cgo| {
        let build = Build::Go {
            packages: vec!["./cmd/foo".into()],
            ldflags: vec!["-X main.version=1.0".into()],
            tags: vec!["netgo".into(), "osusergo".into()],
            cgo,
        };
        scripts(&build, &input(variant)).build.expect("go builds")
    };
    assert_eq!(
        go(Variant::Dynamic, true),
        "mkdir -p \"$BUILDDIR/bin\"\ngo build -buildmode=pie \\\n  \
         -ldflags=\"-linkmode=external -X main.version=1.0\" \\\n  \
         -tags \"netgo,osusergo\" -o \"$BUILDDIR/bin/\" './cmd/foo'"
    );
    assert!(go(Variant::Dynamic, false).contains("-ldflags=\"-X main.version=1.0\""));
    assert!(
        go(Variant::Static, true)
            .contains("go build -buildmode=exe \\\n  -ldflags=\"-extldflags=-static -X")
    );
}

#[test]
fn python_and_kernel_styles_render_their_defaults() {
    let python = scripts(
        &Build::PythonPep517 { check_cmd: None },
        &StyleInput {
            subdir: "py",
            ..input(Variant::Dynamic)
        },
    );
    assert_eq!(python.check, None);
    assert!(
        python
            .build
            .is_some_and(|s| s.ends_with("--outdir \"$BUILDDIR/dist\" \"$SRC/py\""))
    );
    let kernel = |localversion: Option<&str>, flavor: Option<&str>| {
        let build = Build::Kernel {
            config: "/unused".into(),
            localversion: localversion.map(String::from),
            flavor: flavor.map(String::from),
            make_args: vec![],
        };
        scripts(&build, &input(Variant::Dynamic))
            .configure
            .expect("the kernel configures")
    };
    assert_eq!(
        kernel(None, Some("lts")),
        "cp \"/build/recipes/core/foo/files/config\" .config\n\
         make LOCALVERSION=\"-laft3-lts\" olddefconfig"
    );
    assert!(kernel(Some("-custom"), Some("lts")).contains("LOCALVERSION=\"-custom\""));
}
