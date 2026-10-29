//! Checking an evaluated package and converting it into a [`Recipe`].

use std::collections::BTreeMap;
use std::path::Path;

use laft_core::{Blake3Hash, Name, PkgRef, Sha256Hash, Variant};
use stav::{AttrPath, Diagnostic, FileDiagnostic, Priority, Span};

use crate::env::{self, EnvInput, RESERVED};
use crate::loader::{COMMON, RECIPES};
use crate::recipe::{
    Alternative, BuildOptions, Fetch, FileRef, Metadata, Operations, Outputs, Recipe, Source,
    Split, Subpackage, Update, Vendor, VendorKind,
};
use crate::repo::RecipeRepo;
use crate::schema::{self, Build};
use crate::style::{self, StyleInput};
use crate::target::{ARCH, Target};

/// The evaluated package being converted, and the problems found so far.
pub(crate) struct Cx<'a> {
    pub(crate) evaluated: &'a stav::Evaluated,
    pub(crate) name: &'a Name,
    pub(crate) target: Target,
    /// The contributing repositories, lowest priority first.
    pub(crate) repos: &'a [&'a RecipeRepo],
    pub(crate) found: Vec<FileDiagnostic>,
}

impl Cx<'_> {
    /// Reports an error with `code` about the attribute at `keys` below the
    /// package.
    fn error(&mut self, keys: &[&str], code: &'static str, message: String) {
        let path = AttrPath::new(
            ["packages", self.name.as_str()]
                .into_iter()
                .chain(keys.iter().copied()),
        );
        let span = self.evaluated.spans(&path).first().copied();
        self.found.push(FileDiagnostic {
            file: span.map(|s| s.file),
            diagnostic: Diagnostic::error(
                code,
                span.map_or(Span { start: 0, end: 0 }, |s| s.span),
                message,
            ),
        });
    }

    fn has_errors(&self) -> bool {
        !self.found.is_empty()
    }
}

/// Checks the evaluated package `p` and converts it. Returns `None` when a
/// check failed, with the problems in `cx.found`.
pub(crate) fn convert(cx: &mut Cx<'_>, p: schema::Package) -> Option<Recipe> {
    let name = cx.name.as_str().to_owned();
    let variant = cx.target.variant();
    let subdir = p.phases.subdir.clone().unwrap_or_default();
    if !subdir.is_empty() {
        check_relative(cx, &["phases", "subdir"], "phases.subdir", &subdir);
    }

    let sources = sources(cx, &p);
    let vendors = vendors(cx, &p);
    check_package_shape(cx, &p);
    check_env(cx, &p.env);
    if let Some(update) = &p.update {
        check_update(cx, update);
    }
    for suffix in p.subpackages.keys() {
        if format!("{name}-{suffix}").parse::<Name>().is_err() {
            cx.error(
                &["subpackages", suffix],
                "R003",
                format!("subpackage `{name}-{suffix}` is not a valid package name"),
            );
        }
    }
    for (group, alternative) in &p.alternatives {
        if group.parse::<Name>().is_err() {
            cx.error(
                &["alternatives", group],
                "R003",
                format!("alternative group `{group}` is not a valid group name"),
            );
        }
        if let Some(suffix) = &alternative.subpackage
            && !p.subpackages.contains_key(suffix)
            && !p.split.iter().any(|s| s.as_str() == suffix)
        {
            cx.error(
                &["alternatives", group, "subpackage"],
                "R019",
                format!("alternative group `{group}` names subpackage `{name}-{suffix}`, which {name} does not declare"),
            );
        }
    }

    let patches: Vec<FileRef> = p
        .patches
        .iter()
        .enumerate()
        .filter_map(|(i, path)| file_ref(cx, &["patches", &i.to_string()], path))
        .collect();
    let kernel_config = match &p.build {
        Build::Kernel { config, .. } => file_ref(cx, &["build", "kernel", "config"], config),
        _ => None,
    };
    if cx.has_errors() {
        return None;
    }

    let kernel = matches!(p.build, Build::Kernel { .. });
    let check = p.check && !kernel;
    let debug_split = p.debug_split && !kernel;
    let cgo = matches!(p.build, Build::Go { cgo: true, .. });
    let style = style::scripts(
        &p.build,
        &StyleInput {
            name: &name,
            rev: p.rev,
            variant,
            subdir: &subdir,
            check,
            lto: p.lto,
            kernel_config: kernel_config.as_ref().map(|f| f.path.as_str()),
        },
    );
    let phases = style::phases(style, &p.phases, &subdir);

    let mut env = env::environment(&EnvInput {
        name: &name,
        version: p.version.as_str(),
        rev: p.rev,
        target: cx.target,
        repo: cx.repos[0].name.as_str(),
        hardening_disable: &p.hardening_disable,
        lto: p.lto,
        static_pie: p.static_pie,
        cflags: &p.cflags,
        cxxflags: &p.cxxflags,
        ldflags: &p.ldflags,
        rustflags: &p.rustflags,
        goflags: &p.goflags,
        cgo,
    });
    env.extend(p.env.clone());

    let mut build_deps = p.build_deps.clone();
    let mut deps = p.deps.clone();
    if matches!(p.build, Build::PythonPep517 { .. }) {
        for tool in ["python", "python-build", "python-installer", "python-wheel"] {
            add_dep(&mut build_deps, tool);
        }
        add_dep(&mut deps, "python");
    }
    let link_deps = p
        .link_deps
        .iter()
        .map(|r| match variant {
            Variant::Dynamic => r.clone(),
            Variant::Static => PkgRef::new(r.name().clone(), r.prefix().cloned(), variant),
        })
        .collect();

    let mut license = p.license.clone();
    license.sort();
    license.dedup();
    let split = p
        .split
        .iter()
        .copied()
        .filter(|s| variant == Variant::Dynamic || *s == Split::Dbg)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let subpackages = p
        .subpackages
        .iter()
        .map(|(suffix, s)| {
            let subpackage = Subpackage {
                summary: s
                    .summary
                    .clone()
                    .unwrap_or_else(|| format!("{} ({suffix})", p.summary)),
                files: s.files.clone(),
                exclude: s.exclude.clone(),
                deps: s.deps.clone(),
                provides: s.provides.clone(),
                conflicts: s.conflicts.clone(),
                replaces: s.replaces.clone(),
                depends_on_main: s.depends_on_main,
                license: s.license.clone().unwrap_or_else(|| license.clone()),
                keep_empty: s.keep_empty,
            };
            (suffix.clone(), subpackage)
        })
        .collect();
    let alternatives = p
        .alternatives
        .iter()
        .map(|(group, a)| {
            let alternative = Alternative {
                paths: a.paths.clone(),
                priority: a.priority,
                subpackage: a.subpackage.clone(),
            };
            (group.clone(), alternative)
        })
        .collect();

    Some(Recipe {
        name: cx.name.clone(),
        version: p.version,
        rev: p.rev,
        variant,
        repos: cx.repos.iter().map(|r| r.name.clone()).collect(),
        meta: Metadata {
            summary: p.summary,
            description: p.description,
            homepage: p.homepage,
            license,
            license_files: p.license_files,
        },
        sources,
        vendors,
        patches,
        patch_strip: p.patch_strip,
        files: kernel_config.into_iter().collect(),
        build_deps,
        link_deps,
        check_deps: p.check_deps,
        phases,
        env,
        options: BuildOptions {
            check,
            parallel: p.parallel,
            strip: p.strip,
            debug_split,
            static_pie: p.static_pie,
            keep_archives: p.keep_archives,
            lto: p.lto,
            empty: p.empty,
            keep_dirs: p.keep_dirs,
        },
        outputs: Outputs {
            split,
            subpackages,
            alternatives,
            deps,
            provides: p.provides,
            conflicts: p.conflicts,
            replaces: p.replaces,
            ignore_deps: p.ignore_deps,
            setuid: p.setuid,
            caps: p.caps,
            ownership: p.ownership,
        },
        smoke: p.smoke,
        abi_epoch: p.abi_epoch,
        toolchain_epoch: cx.repos.iter().map(|r| r.toolchain_epoch).sum(),
        operations: Operations {
            maintainers: p.maintainers,
            update: p.update,
            cpe: p.cpe,
            ci: p.ci,
        },
    })
}

/// Adds the package `name` to `deps` unless it is there, keeping the
/// canonical order of a set.
fn add_dep(deps: &mut Vec<PkgRef>, name: &str) {
    if !deps.iter().any(|d| d.name().as_str() == name) {
        let dep: PkgRef = name
            .parse()
            .expect("the style's dependencies are valid names");
        deps.push(dep);
        deps.sort_by_key(ToString::to_string);
    }
}

/// Converts the sources, checking every rule a single field cannot express.
fn sources(cx: &mut Cx<'_>, p: &schema::Package) -> Vec<Source> {
    let name = cx.name.to_string();
    let mut out = Vec::new();
    for (label, s) in &p.sources {
        let keys = ["sources", label.as_str()];
        let what = format!("source \"{label}\" of {name}");
        if !is_label(label) {
            cx.error(
                &keys,
                "R018",
                format!("{what} has a label that is not a plain name of letters, digits, `-`, `_` and `.`"),
            );
            continue;
        }
        let dest = s.dest.clone().unwrap_or_else(|| {
            if label == "main" {
                String::new()
            } else {
                label.clone()
            }
        });
        if !dest.is_empty() {
            check_relative(cx, &[keys[0], keys[1], "dest"], "dest", &dest);
        }
        let fetch = match (&s.url, &s.git) {
            (Some(urls), None) => {
                for url in urls {
                    if !url.starts_with("https://") {
                        cx.error(
                            &[keys[0], keys[1], "url"],
                            "R010",
                            format!("{what} uses {url}, but only https:// URLs are allowed"),
                        );
                    }
                }
                let Some(first) = urls.first() else {
                    cx.error(&keys, "R011", format!("{what} has an empty url list"));
                    continue;
                };
                let Some(sha256) = s.sha256.as_deref().and_then(hash) else {
                    cx.error(&keys, "R012", format!("{what} has a url but no sha256"));
                    continue;
                };
                let file = s
                    .file
                    .clone()
                    .unwrap_or_else(|| first.rsplit('/').next().unwrap_or_default().to_owned());
                if !is_file_name(&file) {
                    cx.error(
                        &keys,
                        "R015",
                        format!("{what} has the file name `{file}`, which is not a plain file name; set `file`"),
                    );
                }
                let extract = s
                    .extract
                    .unwrap_or_else(|| crate::extract::is_archive(&file));
                Fetch::Url {
                    urls: urls.clone(),
                    sha256,
                    file,
                    extract,
                }
            }
            (None, Some(git)) => {
                if s.file.is_some() || s.extract.is_some() {
                    cx.error(
                        &keys,
                        "R011",
                        format!("{what} is a git source, but sets `file` or `extract`, which apply to url sources only"),
                    );
                }
                let Some(commit) = s.commit.clone() else {
                    cx.error(&keys, "R013", format!("{what} has a git URL but no commit"));
                    continue;
                };
                Fetch::Git {
                    url: git.clone(),
                    commit,
                    submodules: s.submodules,
                    sha256: s.sha256.as_deref().and_then(hash),
                }
            }
            (Some(_), Some(_)) => {
                cx.error(&keys, "R011", format!("{what} sets both url and git"));
                continue;
            }
            (None, None) => {
                cx.error(&keys, "R011", format!("{what} sets neither url nor git"));
                continue;
            }
        };
        let strip_components = match fetch {
            Fetch::Git { .. } => Some(s.strip_components.unwrap_or(0)),
            Fetch::Url { .. } => s.strip_components,
        };
        out.push(Source {
            label: label.clone(),
            fetch,
            dest,
            strip_components,
        });
    }
    out
}

/// Converts the vendor blocks.
fn vendors(cx: &mut Cx<'_>, p: &schema::Package) -> Vec<Vendor> {
    let mut out = Vec::new();
    for (kind, v) in &p.vendors {
        let keys = ["vendors", kind.as_str()];
        if !p.sources.contains_key(&v.source) {
            cx.error(
                &keys,
                "R014",
                format!(
                    "vendor {} of {} takes its lock file from source \"{}\", which does not exist",
                    kind.as_str(),
                    cx.name,
                    v.source
                ),
            );
        }
        if !v.dir.is_empty() {
            check_relative(cx, &[keys[0], keys[1], "dir"], "dir", &v.dir);
        }
        if let Some(sha256) = hash(&v.sha256) {
            out.push(Vendor {
                kind: *kind,
                sha256,
                dir: v.dir.clone(),
                source: v.source.clone(),
            });
        }
    }
    out
}

/// Checks the rules about sources, vendors and styles that span fields.
fn check_package_shape(cx: &mut Cx<'_>, p: &schema::Package) {
    let name = cx.name.to_string();
    if p.empty {
        if !p.sources.is_empty() {
            cx.error(
                &["sources"],
                "R016",
                format!("{name} is an empty package, which has no sources"),
            );
        }
        if !matches!(p.build, Build::Custom) {
            cx.error(
                &["build"],
                "R016",
                format!("{name} is an empty package, which needs `build custom {{}}`"),
            );
        }
    } else if p.sources.is_empty() {
        cx.error(
            &[],
            "R016",
            format!("{name} has no source; a package without sources sets `empty = true`"),
        );
    }
    let needed = match p.build {
        Build::Cargo { .. } => Some(VendorKind::Cargo),
        Build::Go { .. } => Some(VendorKind::Go),
        _ => None,
    };
    if let Some(kind) = needed
        && !p.vendors.contains_key(&kind)
    {
        let kind = kind.as_str();
        cx.error(
            &["build"],
            "R017",
            format!("{name} uses the {kind} style, which needs a `vendor {kind}` block"),
        );
    }
}

/// Checks the names of the recipe's environment variables. A variable the
/// builder sets may be replaced only with `force(...)`.
fn check_env(cx: &mut Cx<'_>, env: &BTreeMap<String, String>) {
    for key in env.keys() {
        let mut bytes = key.bytes();
        let valid = bytes
            .next()
            .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
            && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !valid {
            cx.error(
                &["env", key],
                "R030",
                format!("environment variable `{key}` does not match [A-Z_][A-Z0-9_]*"),
            );
        } else if RESERVED.contains(&key.as_str())
            && cx.evaluated.priority(&AttrPath::new([
                "packages",
                cx.name.as_str(),
                "env",
                key.as_str(),
            ])) != Some(Priority::Force)
        {
            cx.error(
                &["env", key],
                "R031",
                format!("environment variable `{key}` is set by the builder, and only force(...) replaces it"),
            );
        }
    }
}

/// Checks that exactly one way of finding versions is set, and `pattern`
/// exactly with `url`.
fn check_update(cx: &mut Cx<'_>, update: &Update) {
    let set = [
        update.github.is_some(),
        update.gitlab.is_some(),
        update.anitya.is_some(),
        update.url.is_some(),
    ];
    if set.iter().filter(|s| **s).count() != 1 {
        cx.error(
            &["update"],
            "R050",
            "`update` sets exactly one of github, gitlab, anitya and url".to_owned(),
        );
    }
    if update.url.is_some() != update.pattern.is_some() {
        cx.error(
            &["update"],
            "R050",
            "`update` sets `pattern` exactly when it sets `url`".to_owned(),
        );
    }
}

/// Checks that `path` is a relative path without `.` or `..` components
/// that is safe to place inside double quotes in a script.
fn check_relative(cx: &mut Cx<'_>, keys: &[&str], field: &str, path: &str) {
    let valid = !path.starts_with('/')
        && path
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
        && !path.contains(['"', '$', '`', '\\', '\n']);
    if !valid {
        cx.error(
            keys,
            "R015",
            format!(
                "{field} `{path}` is not a relative path without `.` and `..` components and shell quoting characters"
            ),
        );
    }
}

/// Whether `label` is a source label: one path component of ASCII letters,
/// digits, `-`, `_` and `.` that does not start with `.`.
fn is_label(label: &str) -> bool {
    !label.starts_with('.')
        && !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Whether `file` can be the name of a downloaded file.
fn is_file_name(file: &str) -> bool {
    !file.is_empty()
        && file != "."
        && file != ".."
        && !file.contains(['/', '"', '$', '`', '\\', '\n', '?', '#'])
}

/// Parses a hash that the schema already checked for its form.
fn hash(text: &str) -> Option<Sha256Hash> {
    text.parse().ok()
}

/// Maps the path literal value `path` to its place in the sandbox and
/// hashes the file. The path must lie in the package's recipe directory or
/// in `common/` of one contributing repository.
fn file_ref(cx: &mut Cx<'_>, keys: &[&str], path: &Path) -> Option<FileRef> {
    let name = cx.name.to_string();
    let mapped = cx.repos.iter().find_map(|repo| {
        let rel = path.strip_prefix(&repo.path).ok()?;
        let rel = rel.to_str()?;
        if let Some(rest) = rel
            .strip_prefix(&format!("{RECIPES}/{name}/"))
            .filter(|r| !r.is_empty())
        {
            Some(format!("/build/recipes/{}/{name}/{rest}", repo.name))
        } else {
            let rest = rel.strip_prefix(&format!("{COMMON}/"))?;
            Some(format!("/build/common/{}/{rest}", repo.name))
        }
    });
    let Some(sandbox) = mapped else {
        cx.error(
            keys,
            "R002",
            format!(
                "path {} is outside recipes/{name}/ and common/ of the recipe repository",
                path.display()
            ),
        );
        return None;
    };
    match std::fs::read(path) {
        Ok(bytes) => Some(FileRef {
            path: sandbox,
            blake3: Blake3Hash::of(&bytes),
        }),
        Err(e) => {
            cx.error(
                keys,
                "R006",
                format!("path {} cannot be read: {e}", path.display()),
            );
            None
        }
    }
}

/// Whether the package builds for [`ARCH`]. The descriptor of every
/// contributing repository and the recipe can each restrict it.
pub(crate) fn builds_for_arch(repos: &[&RecipeRepo], arch: Option<&[String]>) -> bool {
    repos.iter().all(|r| r.arch.contains(ARCH)) && arch.is_none_or(|a| a.iter().any(|a| a == ARCH))
}
