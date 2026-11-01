//! Recipe repositories and the evaluation of the packages they define.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use laft_core::{Name, OptionValue, Variant};
use stav::{
    AttrPath, Checked, Diagnostic, FileDiagnostic, ImportRoot, Limits, RootSpec, Span, Value,
};

use crate::convert::{self, Cx};
use crate::host;
use crate::loader::{COMMON, DESCRIPTOR, RECIPE_FILE, RECIPES, Scope};
use crate::options::{self, OPTIONS_MODULE};
use crate::{Error, Problem, Recipe, Report, Target, schema};

/// A recipe repository: a directory with a `recipes.stav` descriptor and a
/// `recipes/<name>/package.stav` file per package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeRepo {
    /// The repository name, the label of its `recipe_repo` block.
    pub name: Name,
    /// The directory, with symbolic links resolved.
    pub path: PathBuf,
    /// The architectures it builds for. A package builds for an
    /// architecture only when every repository that contributes to it
    /// lists it.
    pub arch: BTreeSet<String>,
    /// The repositories it overlays, all earlier in the list.
    pub extends: Vec<Name>,
    /// Incrementing it rebuilds every package the repository contributes
    /// to.
    pub toolchain_epoch: u32,
    /// Content addressed source mirrors, tried after a source's own URLs.
    pub source_mirrors: Vec<String>,
}

/// An ordered set of recipe repositories, lowest priority first, and the
/// packages they define.
///
/// A package is evaluated from its recipe in every repository that has
/// one, as one Stav root in repository order, so a later repository is an
/// overlay that merges into the earlier recipes.
///
/// ```
/// use laft_build::Repos;
///
/// let dir = laft_test::tempdir();
/// let repo = dir.path().join("core");
/// std::fs::create_dir_all(repo.join("recipes/hello"))?;
/// std::fs::write(repo.join("recipes.stav"), "recipe_repo core {}\n")?;
/// std::fs::write(
///     repo.join("recipes/hello/package.stav"),
///     "package hello {\n  version = \"1\"\n  summary = \"Say hello\"\n  license = [MIT]\n  \
///      empty = true\n  build custom {}\n}\n",
/// )?;
///
/// let repos = Repos::open(&[repo]).expect("the repository is valid");
/// let hello = "hello".parse()?;
/// let evaluation = repos.evaluate(&hello).expect("the recipe is valid");
/// assert_eq!(evaluation.recipes[0].meta.summary, "Say hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Repos {
    repos: Vec<RecipeRepo>,
    packages: BTreeMap<Name, Vec<usize>>,
}

/// The recipes of one package, one per variant it builds, and the warnings
/// found while evaluating them.
#[derive(Debug, Clone)]
pub struct Evaluation {
    /// The recipes, `dynamic` before `static`. A package that does not
    /// build for this architecture has none.
    pub recipes: Vec<Recipe>,
    /// The warnings of the language. It holds no errors.
    pub warnings: Report,
}

impl Repos {
    /// Opens the repositories at `paths`, lowest priority first.
    ///
    /// Each descriptor is evaluated, and every directory below `recipes/`
    /// that holds a `package.stav` names a package.
    ///
    /// # Errors
    ///
    /// Returns the [`Report`] of a descriptor that does not evaluate, or
    /// the problems `R004` (a missing or invalid descriptor, a repository
    /// name used twice, or `extends` naming a repository that is not
    /// earlier in the list) and `R003` (a recipe directory whose name is
    /// not a package name).
    pub fn open(paths: &[impl AsRef<Path>]) -> Result<Self, Error> {
        let mut repos: Vec<RecipeRepo> = Vec::new();
        let mut problems = Vec::new();
        let mut packages: BTreeMap<Name, Vec<usize>> = BTreeMap::new();
        for path in paths {
            let path = path.as_ref();
            let real = fs::canonicalize(path).map_err(|e| {
                Error::Problems(vec![Problem::new(
                    "R004",
                    format!("recipe repository {} cannot be opened: {e}", path.display()),
                )])
            })?;
            let repo = descriptor(&real)?;
            if repos.iter().any(|r| r.name == repo.name) {
                problems.push(Problem::new(
                    "R004",
                    format!("recipe repository {} is listed twice", repo.name),
                ));
            }
            for base in &repo.extends {
                if !repos.iter().any(|r| r.name == *base) {
                    problems.push(Problem::new(
                        "R004",
                        format!(
                            "recipe repository {} extends {base}, which is not listed before it",
                            repo.name
                        ),
                    ));
                }
            }
            let index = repos.len();
            for name in recipe_dirs(&real, &mut problems) {
                packages.entry(name).or_default().push(index);
            }
            repos.push(repo);
        }
        if problems.is_empty() {
            Ok(Self { repos, packages })
        } else {
            Err(Error::Problems(problems))
        }
    }

    /// The repositories, lowest priority first.
    pub fn repos(&self) -> &[RecipeRepo] {
        &self.repos
    }

    /// The names of every package, in order.
    pub fn packages(&self) -> impl Iterator<Item = &Name> {
        self.packages.keys()
    }

    /// The host path of the file at the sandbox path `path`, which is below
    /// `/build/recipes/<repo>/` or `/build/common/<repo>/`.
    pub fn host_path(&self, path: &str) -> Option<PathBuf> {
        let (dir, rest) = if let Some(rest) = path.strip_prefix("/build/recipes/") {
            (RECIPES, rest)
        } else {
            ("common", path.strip_prefix("/build/common/")?)
        };
        let (repo, rest) = rest.split_once('/')?;
        let repo = self.repos.iter().find(|r| r.name.as_str() == repo)?;
        let rel = Path::new(rest);
        let safe = rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        safe.then(|| repo.path.join(dir).join(rel))
    }

    /// Evaluates the package `name` for every variant it builds, with
    /// every option at its default.
    ///
    /// The recipe and its overlays are checked once, then evaluated with
    /// the `dynamic` target first, which decides the variants. Every other
    /// variant is evaluated with its own values of the `laft/target`
    /// module and converted.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Problems`] for a package no repository defines, and
    /// otherwise a [`Report`] with every error found, together with the
    /// warnings, when the recipe does not type check or evaluate, or when
    /// it breaks one of the recipe rules of the crate documentation.
    pub fn evaluate(&self, name: &Name) -> Result<Evaluation, Error> {
        self.evaluate_with(name, &BTreeMap::new())
    }

    /// [`Repos::evaluate`] with the option values `set`, which must name
    /// only options the package declares, and for a sum type option only
    /// tags it declares.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Problems`] with `R073` for a set the package does
    /// not declare, and otherwise what [`Repos::evaluate`] returns.
    pub fn evaluate_with(
        &self,
        name: &Name,
        set: &BTreeMap<String, OptionValue>,
    ) -> Result<Evaluation, Error> {
        let Some(indices) = self.packages.get(name) else {
            return Err(Error::Problems(vec![Problem::new(
                "R001",
                format!("no recipe repository defines package {name}"),
            )]));
        };
        let repos: Vec<&RecipeRepo> = indices.iter().map(|&i| &self.repos[i]).collect();
        let all: Vec<&RecipeRepo> = self.repos.iter().collect();
        let (declared, found) = options::declarations(&all, name);
        if !found.is_empty() {
            return Err(Error::Problems(
                found
                    .into_iter()
                    .map(|f| Problem::new(f.diagnostic.code, f.diagnostic.message))
                    .collect(),
            ));
        }
        let bad = match &declared {
            Some(decls) => decls.problems(&format!("the build of {name}"), set),
            None if set.is_empty() => Vec::new(),
            None => vec![format!(
                "the build of {name} sets an option, which the package declares none of"
            )],
        };
        if !bad.is_empty() {
            return Err(Error::Problems(
                bad.into_iter().map(|m| Problem::new("R073", m)).collect(),
            ));
        }
        let roots: Vec<PathBuf> = repos.iter().map(|r| r.path.clone()).collect();
        let mut host = host::host(roots.clone(), Scope::Package(name.to_string()));
        if let Some(decls) = &declared {
            host.host_modules.push(decls.module());
        }
        let spec = RootSpec {
            root_struct: "Recipes".to_owned(),
            entries: roots
                .iter()
                .map(|r| r.join(RECIPES).join(name.as_str()).join(RECIPE_FILE))
                .collect(),
            import_roots: roots.iter().map(ImportRoot::new).collect(),
        };
        let checked = stav::check(&host, &spec).map_err(|e| Error::Report(own_codes(&e)))?;
        let first = evaluate_variant(
            &checked,
            name,
            &repos,
            &all,
            Variant::Dynamic,
            declared.as_ref(),
            set,
        )?;
        let mut recipes = Vec::new();
        let builds = convert::builds_for_arch(&repos, first.arch.as_deref());
        for variant in first.variants.iter().copied().collect::<BTreeSet<_>>() {
            if !builds {
                break;
            }
            let recipe = if variant == Variant::Dynamic {
                first.recipe.clone()
            } else {
                evaluate_variant(
                    &checked,
                    name,
                    &repos,
                    &all,
                    variant,
                    declared.as_ref(),
                    set,
                )?
                .recipe
            };
            recipes.push(recipe);
        }
        let warnings = checked.diagnostics(checked.warnings().iter().cloned());
        Ok(Evaluation { recipes, warnings })
    }
}

/// Evaluates the checked recipe of `name` for `variant`, checks it against
/// the recipe rules and converts it. `repos` are the repositories that
/// contribute to the package and `all` every repository of the run, whose
/// recipes the dependencies may name. `declared` and `set` are the option
/// declarations of the package and the values of this evaluation.
fn evaluate_variant(
    checked: &Checked,
    name: &Name,
    repos: &[&RecipeRepo],
    all: &[&RecipeRepo],
    variant: Variant,
    declared_options: Option<&options::Declarations>,
    set: &BTreeMap<String, OptionValue>,
) -> Result<OneVariant, Error> {
    let target = Target::new(variant);
    let mut values = target.values();
    if let Some(decls) = declared_options {
        decls.apply(&mut values, set);
    }
    // The errors of any step are reported with the warnings of the check.
    let failed = |errors: Vec<FileDiagnostic>| {
        Error::Report(
            checked.diagnostics(errors.into_iter().chain(checked.warnings().iter().cloned())),
        )
    };
    let evaluated = checked
        .eval_with(&Limits::default(), &values)
        .map_err(|e| failed(e.iter().cloned().collect()))?;
    let mut found = Vec::new();
    let declared = match evaluated.value_at(&AttrPath::new(["packages"])) {
        Ok(Value::Map(entries)) => entries.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    };
    for other in declared.iter().filter(|k| *k != name.as_str()) {
        let span = evaluated
            .spans(&AttrPath::new(["packages", other.as_str()]))
            .first()
            .copied();
        found.push(FileDiagnostic {
            file: span.map(|s| s.file),
            diagnostic: Diagnostic::error(
                "R001",
                span.map_or(Span { start: 0, end: 0 }, |s| s.span),
                format!(
                    "{RECIPES}/{name}/{RECIPE_FILE} declares package \"{other}\", expected \"{name}\""
                ),
            ),
        });
    }
    if !declared.iter().any(|k| k == name.as_str()) {
        found.push(FileDiagnostic {
            file: None,
            diagnostic: Diagnostic::error(
                "R001",
                Span { start: 0, end: 0 },
                format!("{RECIPES}/{name}/{RECIPE_FILE} declares no package \"{name}\""),
            ),
        });
    }
    if !found.is_empty() {
        return Err(failed(found));
    }
    let package: schema::Package = evaluated
        .get(&AttrPath::new(["packages", name.as_str()]))
        .map_err(|e| failed(e.iter().cloned().collect()))?;
    let variants = package.variants.clone();
    let arch = package.arch.clone();
    let mut cx = Cx {
        evaluated: &evaluated,
        name,
        target,
        repos,
        all_repos: all,
        found: Vec::new(),
    };
    let recipe = convert::convert(&mut cx, package);
    match recipe {
        Some(mut recipe) if cx.found.is_empty() => {
            if let Some(decls) = declared_options {
                recipe.option_values = decls.resolve(set);
            }
            Ok(OneVariant {
                recipe,
                variants,
                arch,
            })
        }
        _ => Err(failed(cx.found)),
    }
}

/// Reports the codes Stav reports for what a recipe does with its own
/// files and modules as the codes of the recipe rules: an import of a
/// module no host serves as `R070`, and a path or import that leaves the
/// recipe repositories, which Stav reports as `E0702`, as `R002`.
pub(crate) fn own_codes(report: &Report) -> Report {
    report.with_items(report.iter().map(|item| {
        match item.diagnostic.code {
        "E0701" if quoted(&item.diagnostic.message) == Some(OPTIONS_MODULE) => FileDiagnostic {
            file: item.file,
            diagnostic: Diagnostic::error(
                "R070",
                item.diagnostic.span,
                format!("the file imports {OPTIONS_MODULE}, which its package declares no options for"),
            )
            .with_help(format!(
                "declare the options in {RECIPES}/<name>/options.stav, or drop the import"
            )),
        },
        "E0702" => FileDiagnostic {
            file: item.file,
            diagnostic: Diagnostic::error(
                "R002",
                item.diagnostic.span,
                quoted(&item.diagnostic.message).map_or_else(
                    || "the path leaves the recipe repository".to_owned(),
                    |path| format!("the path `{path}` leaves the recipe repository"),
                ),
            )
            .with_help(format!(
                "a recipe reads files only from {RECIPES}/<name>/ and {COMMON}/ of its repository"
            )),
        },
        _ => item.clone(),
    }
    }))
}

/// The first text between backticks in `message`, which is how Stav
/// names the path it reports.
fn quoted(message: &str) -> Option<&str> {
    let (_, rest) = message.split_once('`')?;
    rest.split_once('`').map(|(path, _)| path)
}

/// A recipe evaluated for one variant, with what decides the others.
struct OneVariant {
    recipe: Recipe,
    variants: Vec<Variant>,
    arch: Option<Vec<String>>,
}

/// Evaluates the descriptor of the repository at `path`.
fn descriptor(path: &Path) -> Result<RecipeRepo, Error> {
    let host = host::host(vec![path.to_path_buf()], Scope::Descriptor);
    let spec = RootSpec {
        root_struct: "RecipeRepos".to_owned(),
        entries: vec![path.join(DESCRIPTOR)],
        import_roots: vec![ImportRoot::new(path)],
    };
    let checked = stav::check(&host, &spec).map_err(|e| Error::Report(own_codes(&e)))?;
    let evaluated = checked.eval(&Limits::default()).map_err(|e| {
        Error::Report(checked.diagnostics(e.iter().chain(checked.warnings().iter()).cloned()))
    })?;
    let repos: BTreeMap<String, schema::RecipeRepo> = evaluated
        .get(&AttrPath::new(["repos"]))
        .map_err(Error::Report)?;
    let problem = |message: String| Error::Problems(vec![Problem::new("R004", message)]);
    let mut repos = repos.into_iter();
    let (Some((name, repo)), None) = (repos.next(), repos.next()) else {
        return Err(problem(format!(
            "{} does not declare exactly one recipe_repo block",
            path.join(DESCRIPTOR).display()
        )));
    };
    let name: Name = name
        .parse()
        .map_err(|e| problem(format!("recipe repository name {name} is invalid: {e}")))?;
    let extends = repo
        .extends
        .iter()
        .map(|e| {
            e.parse()
                .map_err(|err| problem(format!("{name} extends {e}, which is invalid: {err}")))
        })
        .collect::<Result<_, _>>()?;
    Ok(RecipeRepo {
        name,
        path: path.to_path_buf(),
        arch: repo.arch.into_iter().collect(),
        extends,
        toolchain_epoch: repo.toolchain_epoch,
        source_mirrors: repo.source_mirrors,
    })
}

/// Returns the names of the recipe directories of the repository `path`.
fn recipe_dirs(path: &Path, problems: &mut Vec<Problem>) -> Vec<Name> {
    let Ok(entries) = fs::read_dir(path.join(RECIPES)) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().join(RECIPE_FILE).is_file() {
            continue;
        }
        let dir = entry.file_name();
        let dir = dir.to_string_lossy();
        match dir.parse::<Name>() {
            Ok(name) => names.push(name),
            Err(e) => problems.push(Problem::new(
                "R003",
                format!("{RECIPES}/{dir} is not named after a package: {e}"),
            )),
        }
    }
    names.sort();
    names
}
