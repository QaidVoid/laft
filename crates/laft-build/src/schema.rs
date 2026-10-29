//! The evaluated recipe schema as Rust values, exactly as Stav renders it.
//! Conversion into [`Recipe`](crate::Recipe) happens in `convert`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use laft_core::{PkgRef, Variant, Version};
use serde::Deserialize;

use crate::recipe::{Ci, Shell, Split, Update, VendorKind};

/// A `recipe_repo` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipeRepo {
    pub(crate) arch: Vec<String>,
    pub(crate) extends: Vec<String>,
    pub(crate) toolchain_epoch: u32,
    pub(crate) source_mirrors: Vec<String>,
}

/// A `package` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Package {
    pub(crate) version: Version,
    pub(crate) rev: u32,
    pub(crate) summary: String,
    pub(crate) description: Option<String>,
    pub(crate) homepage: Option<String>,
    pub(crate) license: Vec<String>,
    pub(crate) license_files: Option<Vec<String>>,
    pub(crate) maintainers: Vec<String>,
    pub(crate) variants: Vec<Variant>,
    pub(crate) arch: Option<Vec<String>>,
    pub(crate) empty: bool,

    pub(crate) sources: BTreeMap<String, Source>,
    pub(crate) vendors: BTreeMap<VendorKind, Vendor>,
    pub(crate) patches: Vec<PathBuf>,
    pub(crate) patch_strip: u32,

    pub(crate) build_deps: Vec<PkgRef>,
    pub(crate) link_deps: Vec<PkgRef>,
    pub(crate) check_deps: Vec<PkgRef>,
    pub(crate) deps: Vec<PkgRef>,
    pub(crate) provides: Vec<String>,
    pub(crate) conflicts: Vec<PkgRef>,
    pub(crate) replaces: Vec<PkgRef>,
    pub(crate) ignore_deps: Vec<String>,

    pub(crate) build: Build,
    pub(crate) shell: Shell,
    pub(crate) phases: Phases,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) cflags: Vec<String>,
    pub(crate) cxxflags: Vec<String>,
    pub(crate) ldflags: Vec<String>,
    pub(crate) rustflags: Vec<String>,
    pub(crate) goflags: Vec<String>,
    pub(crate) hardening_disable: Vec<Hardening>,
    pub(crate) lto: bool,
    pub(crate) check: bool,
    pub(crate) parallel: bool,
    pub(crate) strip: bool,
    pub(crate) debug_split: bool,
    pub(crate) static_pie: bool,
    pub(crate) keep_archives: bool,
    pub(crate) keep_dirs: Vec<String>,
    pub(crate) abi_epoch: u32,

    pub(crate) split: Vec<Split>,
    pub(crate) subpackages: BTreeMap<String, Subpackage>,
    pub(crate) alternatives: BTreeMap<String, Alternative>,
    pub(crate) setuid: Vec<String>,
    pub(crate) caps: BTreeMap<String, String>,
    pub(crate) ownership: BTreeMap<String, String>,

    pub(crate) smoke: Option<String>,
    pub(crate) update: Option<Update>,
    pub(crate) cpe: Vec<String>,
    pub(crate) ci: Ci,
}

/// A `source` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub(crate) url: Option<Vec<String>>,
    pub(crate) git: Option<String>,
    pub(crate) commit: Option<String>,
    pub(crate) submodules: bool,
    pub(crate) sha256: Option<String>,
    pub(crate) file: Option<String>,
    pub(crate) extract: Option<bool>,
    pub(crate) strip_components: Option<u32>,
    pub(crate) dest: Option<String>,
}

/// A `vendor` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Vendor {
    pub(crate) sha256: String,
    pub(crate) dir: String,
    pub(crate) source: String,
}

/// The `phases` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Phases {
    pub(crate) subdir: Option<String>,
    pub(crate) prepare: Option<String>,
    pub(crate) configure: Option<String>,
    pub(crate) build: Option<String>,
    pub(crate) check: Option<String>,
    pub(crate) install: Option<String>,
    pub(crate) post_install: Option<String>,
    pub(crate) pre_configure: Option<String>,
    pub(crate) post_configure: Option<String>,
    pub(crate) pre_build: Option<String>,
    pub(crate) post_build: Option<String>,
    pub(crate) pre_check: Option<String>,
    pub(crate) post_check: Option<String>,
    pub(crate) pre_install: Option<String>,
}

/// A `subpackage` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Subpackage {
    pub(crate) summary: Option<String>,
    pub(crate) files: Vec<String>,
    pub(crate) exclude: Vec<String>,
    pub(crate) deps: Vec<PkgRef>,
    pub(crate) provides: Vec<String>,
    pub(crate) conflicts: Vec<PkgRef>,
    pub(crate) replaces: Vec<PkgRef>,
    pub(crate) depends_on_main: bool,
    pub(crate) license: Option<Vec<String>>,
    pub(crate) keep_empty: bool,
}

/// An `alternative` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Alternative {
    pub(crate) paths: Vec<String>,
    pub(crate) priority: i64,
    pub(crate) subpackage: Option<String>,
}

/// A default hardening flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Hardening {
    Fortify,
    Pie,
    Relro,
    Now,
    StackClash,
    CfProtection,
    Format,
    Plt,
}

/// The meson `auto_features` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Feature {
    Enabled,
    Disabled,
    Auto,
}

/// The build tool cmake generates files for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Generator {
    Ninja,
    Make,
}

/// The build style.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Build {
    Custom,
    Configure {
        args: Vec<String>,
        standard_args: bool,
        autoreconf: bool,
        out_of_tree: bool,
        make_args: Vec<String>,
        check_target: String,
        install_args: Vec<String>,
    },
    Make {
        make_args: Vec<String>,
        check_target: String,
        install_args: Vec<String>,
    },
    Meson {
        options: Vec<String>,
        auto_features: Feature,
    },
    Cmake {
        defines: Vec<String>,
        generator: Generator,
    },
    Cargo {
        features: Vec<String>,
        default_features: bool,
        package: Option<String>,
        bins: Option<Vec<String>>,
        args: Vec<String>,
    },
    Go {
        packages: Vec<String>,
        ldflags: Vec<String>,
        tags: Vec<String>,
        cgo: bool,
    },
    #[serde(rename = "python-pep517")]
    PythonPep517 {
        check_cmd: Option<String>,
    },
    Kernel {
        config: PathBuf,
        localversion: Option<String>,
        flavor: Option<String>,
        make_args: Vec<String>,
    },
}

/// A `stage` block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stage {
    pub(crate) recipes: String,
    pub(crate) install: crate::stage::Install,
    pub(crate) path: Vec<String>,
    pub(crate) steps: Vec<Step>,
}

/// A step of a stage.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    pub(crate) name: String,
    pub(crate) place: crate::stage::Place,
    pub(crate) packages: Vec<PkgRef>,
    pub(crate) script: Option<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) view: Vec<PkgRef>,
}
