//! Rendering build styles and phase hooks into shell scripts. Every style
//! renders POSIX shell, so a recipe needs `bash` only for its own scripts.
//!
//! The templates here are the definition of each style. Changing what one
//! renders changes the plan hash of every recipe using the style, which
//! rebuilds them.

use laft_core::Variant;

use crate::recipe::{Phases, Shell};
use crate::schema::{self, Build, Feature, Generator};

/// The style's own scripts for each phase, before hooks are added.
#[derive(Debug, Default)]
pub(crate) struct Scripts {
    pub(crate) prepare: Option<String>,
    pub(crate) configure: Option<String>,
    pub(crate) build: Option<String>,
    pub(crate) check: Option<String>,
    pub(crate) install: Option<String>,
}

/// What a style needs to know about the package besides its own fields.
pub(crate) struct StyleInput<'a> {
    pub(crate) name: &'a str,
    pub(crate) rev: u32,
    pub(crate) variant: Variant,
    /// `phases.subdir`, or `""`.
    pub(crate) subdir: &'a str,
    pub(crate) check: bool,
    pub(crate) lto: bool,
    /// The sandbox path of the kernel configuration.
    pub(crate) kernel_config: Option<&'a str>,
}

/// Returns `text` as one single quoted shell word.
pub(crate) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Returns `text` escaped for use inside a double quoted shell word.
fn dq(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Returns the source directory `$SRC/<subdir>`, or `$SRC` without a
/// subdirectory, for use inside a double quoted shell word.
fn src_dir(subdir: &str) -> String {
    if subdir.is_empty() {
        "$SRC".to_owned()
    } else {
        format!("$SRC/{}", dq(subdir))
    }
}

/// Returns every item as a single quoted word.
fn words(items: &[String]) -> Vec<String> {
    items.iter().map(|s| quote(s)).collect()
}

/// Joins the non-empty words of a line with spaces.
fn line(parts: impl IntoIterator<Item = impl Into<String>>) -> String {
    let parts: Vec<String> = parts.into_iter().map(Into::into).collect();
    let parts: Vec<&str> = parts
        .iter()
        .map(String::as_str)
        .filter(|p| !p.is_empty())
        .collect();
    parts.join(" ")
}

/// Joins the non-empty lines of one command with `\` continuations, each
/// continuation line indented by two spaces.
fn command(lines: impl IntoIterator<Item = String>) -> String {
    let lines: Vec<String> = lines.into_iter().filter(|l| !l.is_empty()).collect();
    lines.join(" \\\n  ")
}

fn boolean(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// Returns the scripts of the style `build`.
pub(crate) fn scripts(build: &Build, input: &StyleInput<'_>) -> Scripts {
    let dynamic = input.variant == Variant::Dynamic;
    let src = src_dir(input.subdir);
    match build {
        Build::Custom => Scripts::default(),
        Build::Configure {
            args,
            standard_args,
            autoreconf,
            out_of_tree,
            make_args,
            check_target,
            install_args,
        } => {
            let enter = if *out_of_tree {
                "cd \"$BUILDDIR\"\n"
            } else {
                ""
            };
            let standard = match (*standard_args, dynamic) {
                (false, _) => "",
                (true, true) => STANDARD_ARGS_DYNAMIC,
                (true, false) => STANDARD_ARGS_STATIC,
            };
            let configure = line(
                [format!("\"{src}/configure\""), standard.into()]
                    .into_iter()
                    .chain(words(args)),
            );
            Scripts {
                prepare: autoreconf.then(|| "autoreconf -fi".to_owned()),
                configure: Some(format!("{enter}{configure}")),
                build: Some(format!(
                    "{enter}{}",
                    line(["make".to_owned()].into_iter().chain(words(make_args)))
                )),
                check: Some(format!("{enter}make {}", quote(check_target))),
                install: Some(format!(
                    "{enter}{}",
                    line(
                        ["make DESTDIR=\"$DESTDIR\"".to_owned()]
                            .into_iter()
                            .chain(words(install_args))
                            .chain(["install".to_owned()])
                    )
                )),
            }
        }
        Build::Make {
            make_args,
            check_target,
            install_args,
        } => Scripts {
            build: Some(line(
                ["make PREFIX=/usr".to_owned()]
                    .into_iter()
                    .chain(words(make_args)),
            )),
            check: Some(format!("make PREFIX=/usr {}", quote(check_target))),
            install: Some(line(
                ["make DESTDIR=\"$DESTDIR\" PREFIX=/usr".to_owned()]
                    .into_iter()
                    .chain(words(install_args))
                    .chain(["install".to_owned()]),
            )),
            ..Scripts::default()
        },
        Build::Meson {
            options,
            auto_features,
        } => {
            let features = match auto_features {
                Feature::Enabled => "enabled",
                Feature::Disabled => "disabled",
                Feature::Auto => "auto",
            };
            let library = if dynamic {
                "--default-library=shared"
            } else {
                "--default-library=static --prefer-static --cross-file=\"$LAFT_CROSS_DIR/meson.ini\""
            };
            Scripts {
                configure: Some(command([
                    format!("meson setup \"$BUILDDIR\" \"{src}\""),
                    "--prefix=/usr --libexecdir=lib --sbindir=bin --sysconfdir=/etc --localstatedir=/var".into(),
                    format!("--buildtype=plain --wrap-mode=nodownload --auto-features={features}"),
                    format!("-Db_pie=true -Db_lto={} -Dpython.bytecompile=1", boolean(input.lto)),
                    library.into(),
                    line(words(options)),
                ])),
                build: Some("meson compile -C \"$BUILDDIR\" -j \"$JOBS\"".into()),
                check: Some(
                    "meson test -C \"$BUILDDIR\" --print-errorlogs --num-processes \"$JOBS\""
                        .into(),
                ),
                install: Some(
                    "meson install -C \"$BUILDDIR\" --destdir \"$DESTDIR\" --no-rebuild".into(),
                ),
                ..Scripts::default()
            }
        }
        Build::Cmake { defines, generator } => {
            let generator = match generator {
                Generator::Ninja => "Ninja",
                Generator::Make => "\"Unix Makefiles\"",
            };
            let shared = if dynamic {
                "-DBUILD_SHARED_LIBS=ON"
            } else {
                "-DBUILD_SHARED_LIBS=OFF -DCMAKE_TOOLCHAIN_FILE=\"$LAFT_CROSS_DIR/toolchain.cmake\""
            };
            Scripts {
                configure: Some(command([
                    format!("cmake -S \"{src}\" -B \"$BUILDDIR\" -G {generator}"),
                    "-DCMAKE_BUILD_TYPE=None -DCMAKE_INSTALL_PREFIX=/usr".into(),
                    "-DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_INSTALL_SBINDIR=bin -DCMAKE_INSTALL_LIBEXECDIR=lib".into(),
                    format!(
                        "-DCMAKE_INSTALL_SYSCONFDIR=/etc -DBUILD_TESTING={}",
                        boolean(input.check)
                    ),
                    shared.into(),
                    line(words(defines)),
                ])),
                build: Some("cmake --build \"$BUILDDIR\" --parallel \"$JOBS\"".into()),
                check: Some(
                    "ctest --test-dir \"$BUILDDIR\" --output-on-failure -j \"$JOBS\"".into(),
                ),
                install: Some("DESTDIR=\"$DESTDIR\" cmake --install \"$BUILDDIR\"".into()),
                ..Scripts::default()
            }
        }
        Build::Cargo {
            features,
            default_features,
            package,
            bins,
            args,
        } => {
            let package = package
                .as_ref()
                .map(|p| format!("-p {}", quote(p)))
                .unwrap_or_default();
            let no_default = if *default_features {
                ""
            } else {
                "--no-default-features"
            };
            let features = if features.is_empty() {
                String::new()
            } else {
                format!("--features {}", quote(&features.join(",")))
            };
            let bins = match bins {
                Some(bins) => words(bins).join(" "),
                None => quote(input.name),
            };
            let cargo = "--frozen --release --target \"$CARGO_TARGET\"";
            Scripts {
                prepare: Some(CARGO_PREPARE.into()),
                build: Some(command([
                    format!("cargo build {cargo}"),
                    line([package.as_str(), no_default]),
                    line([features.clone()].into_iter().chain(words(args))),
                ])),
                check: Some(line([
                    format!("cargo test {cargo}"),
                    package,
                    no_default.into(),
                    features,
                ])),
                install: Some(format!(
                    "for b in {bins}; do install -Dm755 \"$CARGO_OUT/$b\" \"$DESTDIR/usr/bin/$b\"; done"
                )),
                ..Scripts::default()
            }
        }
        Build::Go {
            packages,
            ldflags,
            tags,
            cgo,
        } => {
            let mode = if dynamic {
                "-buildmode=pie"
            } else {
                "-buildmode=exe"
            };
            let ld = line([
                if *cgo && dynamic {
                    "-linkmode=external".to_owned()
                } else {
                    String::new()
                },
                if dynamic {
                    String::new()
                } else {
                    "-extldflags=-static".to_owned()
                },
                dq(&ldflags.join(" ")),
            ]);
            let packages = words(packages).join(" ");
            Scripts {
                prepare: Some(format!("cp -a /build/vendor/go \"{src}/vendor\"")),
                build: Some(format!(
                    "mkdir -p \"$BUILDDIR/bin\"\n{}",
                    command([
                        format!("go build {mode}"),
                        format!("-ldflags=\"{ld}\""),
                        line([
                            format!("-tags \"{}\" -o \"$BUILDDIR/bin/\"", dq(&tags.join(","))),
                            packages.clone(),
                        ]),
                    ])
                )),
                check: Some(line(["go test".to_owned(), packages])),
                install: Some("install -Dm755 -t \"$DESTDIR/usr/bin\" \"$BUILDDIR\"/bin/*".into()),
                ..Scripts::default()
            }
        }
        Build::PythonPep517 { check_cmd } => Scripts {
            build: Some(command([
                "python3 -m build --wheel --no-isolation --skip-dependency-check".to_owned(),
                format!("--outdir \"$BUILDDIR/dist\" \"{src}\""),
            ])),
            check: check_cmd.clone(),
            install: Some(
                "python3 -m installer --destdir=\"$DESTDIR\" --compile-bytecode=1 \"$BUILDDIR\"/dist/*.whl"
                    .into(),
            ),
            ..Scripts::default()
        },
        Build::Kernel {
            localversion,
            flavor,
            make_args,
            ..
        } => {
            let local = dq(&localversion.clone().unwrap_or_else(|| match flavor {
                Some(flavor) => format!("-laft{}-{flavor}", input.rev),
                None => format!("-laft{}", input.rev),
            }));
            let config = dq(input.kernel_config.unwrap_or_default());
            Scripts {
                configure: Some(format!(
                    "cp \"{config}\" .config\nmake LOCALVERSION=\"{local}\" olddefconfig"
                )),
                build: Some(command([
                    format!(
                        "make LOCALVERSION=\"{local}\" KBUILD_BUILD_TIMESTAMP=\"@$SOURCE_DATE_EPOCH\""
                    ),
                    line(
                        ["KBUILD_BUILD_USER=laft KBUILD_BUILD_HOST=laft".to_owned()]
                            .into_iter()
                            .chain(words(make_args))
                            .chain(["bzImage modules".to_owned()]),
                    ),
                ])),
                check: None,
                install: Some(format!(
                    "kver=\"$(make -s LOCALVERSION=\"{local}\" kernelrelease)\"\n\
                     install -Dm644 arch/x86/boot/bzImage \"$DESTDIR/usr/lib/modules/$kver/vmlinuz\"\n\
                     make LOCALVERSION=\"{local}\" INSTALL_MOD_PATH=\"$DESTDIR/usr\" \\\n  \
                     INSTALL_MOD_STRIP=1 modules_install\n\
                     rm -f \"$DESTDIR/usr/lib/modules/$kver/build\"\n\
                     install -Dm644 .config \"$DESTDIR/usr/lib/modules/$kver/config\"\n\
                     install -Dm644 System.map \"$DESTDIR/usr/lib/modules/$kver/System.map\""
                )),
                ..Scripts::default()
            }
        }
    }
}

const STANDARD_ARGS_DYNAMIC: &str = "--prefix=/usr --sysconfdir=/etc --localstatedir=/var \
    --bindir=/usr/bin --sbindir=/usr/bin --libdir=/usr/lib --libexecdir=/usr/lib \
    --mandir=/usr/share/man --infodir=/usr/share/info \
    --build=$CBUILD --host=$CHOST --disable-static --enable-shared";

const STANDARD_ARGS_STATIC: &str = "--prefix=/usr --sysconfdir=/etc --localstatedir=/var \
    --bindir=/usr/bin --sbindir=/usr/bin --libdir=/usr/lib --libexecdir=/usr/lib \
    --mandir=/usr/share/man --infodir=/usr/share/info \
    --build=$CBUILD --host=$CHOST --enable-static --disable-shared";

const CARGO_PREPARE: &str = "mkdir -p \"$CARGO_HOME\"
cat > \"$CARGO_HOME/config.toml\" <<'EOF'
[source.crates-io]
replace-with = \"vendored\"
[source.vendored]
directory = \"/build/vendor/cargo\"
[net]
offline = true
EOF";

/// Folds the recipe's replacements and hooks into the style's scripts and
/// adds the common header to every phase that runs anything.
pub(crate) fn phases(
    style: Scripts,
    recipe: &schema::Phases,
    subdir: &str,
    shell: Shell,
) -> Phases {
    let header = format!("{}umask 022\ncd \"{}\"\n", shell.header(), src_dir(subdir));
    let render = |pre: Option<&String>, script: Option<String>, post: Option<&String>| {
        let parts: Vec<&str> = [
            pre.map(String::as_str),
            script.as_deref(),
            post.map(String::as_str),
        ]
        .into_iter()
        .flatten()
        .map(|p| p.trim_end_matches('\n'))
        .filter(|p| !p.trim().is_empty())
        .collect();
        (!parts.is_empty()).then(|| format!("{header}{}\n", parts.join("\n")))
    };
    let pick = |replacement: &Option<String>, style: Option<String>| match replacement {
        Some(script) => Some(script.clone()),
        None => style,
    };
    Phases {
        prepare: render(None, pick(&recipe.prepare, style.prepare), None),
        configure: render(
            recipe.pre_configure.as_ref(),
            pick(&recipe.configure, style.configure),
            recipe.post_configure.as_ref(),
        ),
        build: render(
            recipe.pre_build.as_ref(),
            pick(&recipe.build, style.build),
            recipe.post_build.as_ref(),
        ),
        check: render(
            recipe.pre_check.as_ref(),
            pick(&recipe.check, style.check),
            recipe.post_check.as_ref(),
        ),
        install: render(
            recipe.pre_install.as_ref(),
            pick(&recipe.install, style.install),
            None,
        ),
        post_install: render(None, recipe.post_install.clone(), None),
    }
}

#[cfg(test)]
#[path = "style_tests.rs"]
mod tests;
