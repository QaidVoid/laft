//! Recipe repositories and the evaluation of the packages they define.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use laft_core::{Name, Variant};
use stav::{AttrPath, Diagnostic, FileDiagnostic, ImportRoot, Limits, RootSpec, Span, Value};

use crate::convert::{self, Cx};
use crate::host;
use crate::loader::{DESCRIPTOR, RECIPE_FILE, RECIPES, Scope};
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

    /// Evaluates the package `name` for every variant it builds.
    ///
    /// The recipe and its overlays are evaluated with the `dynamic` target
    /// first, which decides the variants. Every variant is then evaluated
    /// with its own `laft/target` module, checked and converted.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Problems`] for a package no repository defines, and
    /// otherwise a [`Report`] with every error found, together with the
    /// warnings, when the recipe does not type check or evaluate, or when
    /// it breaks one of the recipe rules of the crate documentation.
    pub fn evaluate(&self, name: &Name) -> Result<Evaluation, Error> {
        let Some(indices) = self.packages.get(name) else {
            return Err(Error::Problems(vec![Problem::new(
                "R001",
                format!("no recipe repository defines package {name}"),
            )]));
        };
        let repos: Vec<&RecipeRepo> = indices.iter().map(|&i| &self.repos[i]).collect();
        // Every variant reads the same files, so the warnings of the first
        // evaluation stand for all of them.
        let (first, warnings) = self.evaluate_variant(name, &repos, Variant::Dynamic)?;
        let mut recipes = Vec::new();
        let builds = convert::builds_for_arch(&repos, first.arch.as_deref());
        for variant in first.variants.iter().copied().collect::<BTreeSet<_>>() {
            if !builds {
                break;
            }
            let recipe = if variant == Variant::Dynamic {
                first.recipe.clone()
            } else {
                self.evaluate_variant(name, &repos, variant)?.0.recipe
            };
            recipes.push(recipe);
        }
        let warnings = first.evaluated.diagnostics(warnings);
        Ok(Evaluation { recipes, warnings })
    }

    fn evaluate_variant(
        &self,
        name: &Name,
        repos: &[&RecipeRepo],
        variant: Variant,
    ) -> Result<(OneVariant, Vec<FileDiagnostic>), Error> {
        let target = Target::new(variant);
        let roots: Vec<PathBuf> = repos.iter().map(|r| r.path.clone()).collect();
        let host = host::host(roots.clone(), Scope::Package(name.to_string()), target);
        let spec = RootSpec {
            root_struct: "Recipes".to_owned(),
            entries: roots
                .iter()
                .map(|r| r.join(RECIPES).join(name.as_str()).join(RECIPE_FILE))
                .collect(),
            import_roots: roots.iter().map(ImportRoot::new).collect(),
        };
        let checked = stav::check(&host, &spec).map_err(Error::Report)?;
        let evaluated = checked.eval(&Limits::default()).map_err(Error::Report)?;
        let warnings: Vec<FileDiagnostic> = checked.warnings().iter().cloned().collect();
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
            return Err(Error::Report(
                evaluated.diagnostics(found.into_iter().chain(warnings)),
            ));
        }
        let package: schema::Package = evaluated
            .get(&AttrPath::new(["packages", name.as_str()]))
            .map_err(Error::Report)?;
        let variants = package.variants.clone();
        let arch = package.arch.clone();
        let mut cx = Cx {
            evaluated: &evaluated,
            name,
            target,
            repos,
            found: Vec::new(),
        };
        let recipe = convert::convert(&mut cx, package);
        let found = cx.found;
        match recipe {
            Some(recipe) if found.is_empty() => Ok((
                OneVariant {
                    recipe,
                    variants,
                    arch,
                    evaluated,
                },
                warnings,
            )),
            _ => Err(Error::Report(
                evaluated.diagnostics(found.into_iter().chain(warnings)),
            )),
        }
    }
}

/// A recipe evaluated for one variant, with what decides the others.
struct OneVariant {
    recipe: Recipe,
    variants: Vec<Variant>,
    arch: Option<Vec<String>>,
    evaluated: stav::Evaluated,
}

/// Evaluates the descriptor of the repository at `path`.
fn descriptor(path: &Path) -> Result<RecipeRepo, Error> {
    let host = host::host(
        vec![path.to_path_buf()],
        Scope::Descriptor,
        Target::new(Variant::Dynamic),
    );
    let spec = RootSpec {
        root_struct: "RecipeRepos".to_owned(),
        entries: vec![path.join(DESCRIPTOR)],
        import_roots: vec![ImportRoot::new(path)],
    };
    let evaluated = stav::check(&host, &spec)
        .and_then(|c| c.eval(&Limits::default()))
        .map_err(Error::Report)?;
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
