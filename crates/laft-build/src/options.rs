//! The options a recipe declares: `options.stav`, the host module
//! `laft/options` and the checks of both.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use laft_core::{MAX_OPTIONS, Name, OptionConstraint, OptionValue, is_valid_name};
use stav::ast::{self, Expr, Field, Item, LiteralKind, StructDecl, Type, TypeDecl};
use stav::{Diagnostic, FileDiagnostic, HostModule, HostValues, Span, Value, parse};

use crate::RecipeRepo;
use crate::host::PRELUDE;

/// The import path of the options module.
pub(crate) const OPTIONS_MODULE: &str = "laft/options";

/// The name every options file gives its struct.
const STRUCT_NAME: &str = "Options";

/// The first line of a field's doc comment, at most this many characters,
/// is what the commands show.
const DOC_LINE_CHARS: usize = 80;

/// The type of one option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `Bool`.
    Bool,
    /// The sum type of the tags, declared in the same file.
    Sum(String),
}

/// One declared option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declaration {
    /// The name.
    pub(crate) name: String,
    /// The type.
    pub(crate) kind: Kind,
    /// The default.
    pub(crate) default: OptionValue,
    /// The first line of the doc comment.
    pub(crate) doc: String,
}

/// A declared sum type of bare tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sum {
    /// The name.
    pub(crate) name: String,
    /// The tags, in declaration order.
    pub(crate) tags: Vec<String>,
}

/// Every option a package declares: the sum types and fields of the
/// `options.stav` files of every repository that contributes to it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Declarations {
    sums: Vec<Sum>,
    fields: Vec<Declaration>,
}

impl Declarations {
    /// The value of every option when nothing is set.
    pub(crate) fn defaults(&self) -> BTreeMap<String, OptionValue> {
        self.fields
            .iter()
            .map(|f| (f.name.clone(), f.default.clone()))
            .collect()
    }

    /// What stops `set` from being a valid option set of this package.
    pub(crate) fn problems(&self, what: &str, set: &BTreeMap<String, OptionValue>) -> Vec<String> {
        let mut found = Vec::new();
        for (name, value) in set {
            let Some(field) = self.fields.iter().find(|f| f.name == *name) else {
                found.push(format!(
                    "{what} sets the option `{name}`, which it does not declare"
                ));
                continue;
            };
            match (&field.kind, value) {
                (Kind::Bool, OptionValue::Bool(_)) => {}
                (Kind::Bool, OptionValue::Tag(tag)) => found.push(format!(
                    "{what} sets the option `{name}` to the tag `{tag}`, which a `Bool` option does not take"
                )),
                (Kind::Sum(_), OptionValue::Bool(bool)) => found.push(format!(
                    "{what} sets the option `{name}` to `{bool}`, which a tag option does not take"
                )),
                (Kind::Sum(sum), OptionValue::Tag(tag)) => {
                    if !self
                        .sums
                        .iter()
                        .find(|s| &s.name == sum)
                        .is_some_and(|s| s.tags.iter().any(|t| t == tag))
                    {
                        found.push(format!(
                            "{what} sets the option `{name}` to `{tag}`, which is no tag of `{sum}`"
                        ));
                    }
                }
            }
        }
        found
    }

    /// What a dependency constraint on this package is missing, or `None`
    /// when the package satisfies `constraint`.
    pub(crate) fn constraint_problem(
        &self,
        target: &Name,
        constraint: &OptionConstraint,
    ) -> Option<String> {
        let name = constraint.name();
        let Some(field) = self.fields.iter().find(|f| f.name == name) else {
            return Some(format!(
                "the dependency on {target} requires the option `{name}`, which {target} does not declare"
            ));
        };
        match constraint {
            OptionConstraint::Tag { tag, .. } => match &field.kind {
                Kind::Bool => Some(format!(
                    "the dependency on {target} requires `{name}={tag}`, which is a `Bool` option"
                )),
                Kind::Sum(sum)
                    if self
                        .sums
                        .iter()
                        .find(|s| &s.name == sum)
                        .is_some_and(|s| s.tags.iter().any(|t| t == tag)) =>
                {
                    None
                }
                Kind::Sum(sum) => Some(format!(
                    "the dependency on {target} requires `{name}={tag}`, which is no tag of `{sum}`"
                )),
            },
            OptionConstraint::True { .. } | OptionConstraint::False { .. }
                if matches!(field.kind, Kind::Bool) =>
            {
                None
            }
            OptionConstraint::True { .. } | OptionConstraint::False { .. } => Some(format!(
                "the dependency on {target} requires `{name}` as a bool, which is a tag option"
            )),
        }
    }

    /// Every value of `set` resolved over the defaults, so the map holds
    /// every declared option.
    ///
    /// # Panics
    ///
    /// Panics when `set` names an option or a tag the package does not
    /// declare, which [`Declarations::problems`] rules out.
    pub(crate) fn resolve(
        &self,
        set: &BTreeMap<String, OptionValue>,
    ) -> BTreeMap<String, OptionValue> {
        assert!(
            self.problems("the build", set).is_empty(),
            "the set names a declared option and tag"
        );
        let mut values = self.defaults();
        for (name, value) in set {
            values.insert(name.clone(), value.clone());
        }
        values
    }

    /// The values of the `laft/options` module for one evaluation, set on
    /// top of `values`, which already holds the values of the other host
    /// modules. A value replaces the binding the module declares.
    ///
    /// # Panics
    ///
    /// Panics when `set` names an option the package does not declare, or
    /// a value of a kind the option does not take, which
    /// [`Declarations::problems`] rules out.
    pub(crate) fn apply(&self, values: &mut HostValues, set: &BTreeMap<String, OptionValue>) {
        for (name, value) in set {
            let value = match value {
                OptionValue::Bool(b) => Value::Bool(*b),
                OptionValue::Tag(tag) => {
                    let field = self
                        .fields
                        .iter()
                        .find(|f| f.name == *name)
                        .expect("the set names a declared option");
                    let Kind::Sum(sum) = &field.kind else {
                        unreachable!("a tag is only set for a sum type option");
                    };
                    Value::Variant {
                        ty: sum.clone(),
                        name: tag.clone(),
                        fields: None,
                    }
                }
            };
            values.insert(OPTIONS_MODULE, name.clone(), value);
        }
    }

    /// The Stav source of the `laft/options` module: the sum types, the
    /// `Options` struct and one annotated binding per option, which one
    /// evaluation replaces with [`Declarations::values`]. A tag default is
    /// written qualified as `Sum.tag`, so a tag that shares its name with
    /// a variant of another sum type cannot be read as that one.
    pub(crate) fn module(&self) -> HostModule {
        let mut source = String::new();
        for sum in &self.sums {
            source.push_str(&format!("type {} = {}\n", sum.name, sum.tags.join(" | ")));
        }
        source.push_str("struct Options {\n");
        for field in &self.fields {
            source.push_str(&format!(
                "  ## {}\n  {}: {} = {}\n",
                field.doc,
                field.name,
                type_text(&field.kind),
                default_text(&field.kind, &field.default),
            ));
        }
        source.push_str("}\n");
        for field in &self.fields {
            source.push_str(&format!(
                "## {}\nlet {}: {} = {}\n",
                field.doc,
                field.name,
                type_text(&field.kind),
                default_text(&field.kind, &field.default),
            ));
        }
        HostModule {
            path: OPTIONS_MODULE.to_owned(),
            source,
        }
    }
}

/// The Stav name of a declared type.
fn type_text(kind: &Kind) -> &str {
    match kind {
        Kind::Bool => "Bool",
        Kind::Sum(name) => name,
    }
}

/// The source text of a default: a tag qualified by its sum type, so a
/// tag that shadows a variant of another sum type stays unambiguous.
fn default_text(kind: &Kind, default: &OptionValue) -> String {
    match (kind, default) {
        (Kind::Sum(sum), OptionValue::Tag(tag)) => format!("{sum}.{tag}"),
        _ => default.to_string(),
    }
}

/// The type names a sum type in `options.stav` cannot take, with what
/// each name collides with: `Options`, which the generated module
/// declares, the built-in types of the recipe language, and the types its
/// prelude declares, which every file sees unqualified.
fn reserved_type_name(name: &str) -> Option<&'static str> {
    if name == STRUCT_NAME {
        return Some("the struct of the options module");
    }
    if stav::is_builtin_type_name(name) {
        return Some("a built-in type");
    }
    if prelude_type_names().contains(name) {
        return Some("a type of the recipe language");
    }
    None
}

/// Every type name the recipe prelude declares, parsed once.
fn prelude_type_names() -> &'static BTreeSet<String> {
    static NAMES: OnceLock<BTreeSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let parsed = parse(PRELUDE);
        parsed
            .file()
            .items()
            .filter_map(|item| match item {
                Item::Type(decl) => decl.name().map(|n| n.text().to_owned()),
                Item::Struct(decl) => decl.name().map(|n| n.text().to_owned()),
                _ => None,
            })
            .collect()
    })
}

/// Reads the `options.stav` of every repository that contributes to
/// `name`, lowest priority first, and merges them. Returns `None` when no
/// repository has one. Every file that breaks a rule is reported as
/// `R072`, and an overlay that declares a sum type or option again as
/// `R071`.
pub(crate) fn declarations(
    repos: &[&RecipeRepo],
    name: &Name,
) -> (Option<Declarations>, Vec<FileDiagnostic>) {
    let mut found = Vec::new();
    let mut sums: Vec<Sum> = Vec::new();
    let mut fields: Vec<Declaration> = Vec::new();
    let mut any = false;
    for repo in repos {
        let file = repo
            .path
            .join("recipes")
            .join(name.as_str())
            .join("options.stav");
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                found.push(problem(
                    "R072",
                    format!(
                        "recipes/{}/options.stav of repository {} cannot be read: {e}",
                        name.as_str(),
                        repo.name
                    ),
                ));
                continue;
            }
        };
        any = true;
        let file = format!(
            "recipes/{}/options.stav of repository {}",
            name.as_str(),
            repo.name
        );
        let mut repo_sums = Vec::new();
        let mut repo_fields: Vec<Declaration> = Vec::new();
        read_file(
            &file,
            &text,
            &sums,
            &mut repo_sums,
            &mut repo_fields,
            &mut found,
        );
        for sum in repo_sums {
            if sums.iter().any(|s| s.name == sum.name) {
                found.push(problem(
                    "R071",
                    format!(
                        "repository {} declares the sum type `{}` again, which an earlier repository of {name} already declares",
                        repo.name, sum.name
                    ),
                ));
                continue;
            }
            sums.push(sum);
        }
        for field in repo_fields {
            if fields.iter().any(|f| f.name == field.name) {
                found.push(problem(
                    "R071",
                    format!(
                        "repository {} declares the option `{}` again, which an earlier repository of {name} already declares",
                        repo.name, field.name
                    ),
                ));
                continue;
            }
            fields.push(field);
        }
    }
    if !any {
        return (None, found);
    }
    if fields.len() > MAX_OPTIONS {
        found.push(problem(
            "R072",
            format!("a recipe declares at most {MAX_OPTIONS} options"),
        ));
    }
    (Some(Declarations { sums, fields }), found)
}

/// Checks one `options.stav` and adds its sum types and fields. `file`
/// names the file in the diagnostics and `text` is its source. `known`
/// holds the sum types of the earlier repositories, which the fields of
/// this file may use too.
fn read_file(
    file: &str,
    text: &str,
    known: &[Sum],
    sums: &mut Vec<Sum>,
    fields: &mut Vec<Declaration>,
    found: &mut Vec<FileDiagnostic>,
) {
    let parsed = parse(text);
    if !parsed.diagnostics.is_empty() {
        for diagnostic in &parsed.diagnostics {
            found.push(problem_at(
                file,
                text,
                diagnostic.span,
                "R072",
                format!("options.stav does not parse: {}", diagnostic.message),
            ));
        }
        return;
    }
    let mut own_sums = Vec::new();
    let mut structs = Vec::new();
    for item in parsed.file().items() {
        match item {
            Item::Type(decl) => read_sum(file, text, decl, &mut own_sums, found),
            Item::Struct(decl) => structs.push(decl),
            Item::Import(_) => found.push(problem_in(
                file,
                "R072",
                "options.stav holds no imports: the options read nothing else",
            )),
            Item::Fn(_) | Item::Stmt(_) => found.push(problem_in(
                file,
                "R072",
                "options.stav holds only sum types and the struct `Options`",
            )),
        }
    }
    let mut all_sums: Vec<&Sum> = own_sums.iter().chain(known.iter()).collect();
    for decl in &structs {
        if decl.name().is_some_and(|n| n.text() == STRUCT_NAME) {
            read_fields(file, text, *decl, &mut all_sums, fields, found);
        } else {
            let problem = match decl.name() {
                Some(name) => problem_at(
                    file,
                    text,
                    name.span(),
                    "R072",
                    format!(
                        "the struct `{}` does not belong in options.stav: only `{STRUCT_NAME}` and sum types do",
                        name.text()
                    ),
                ),
                None => problem_in(
                    file,
                    "R072",
                    format!(
                        "a struct without a name does not belong in options.stav: only `{STRUCT_NAME}` and sum types do"
                    ),
                ),
            };
            found.push(problem);
        }
    }
    if structs.is_empty() {
        found.push(problem_in(
            file,
            "R072",
            format!("options.stav declares no struct `{STRUCT_NAME}`"),
        ));
    }
    if structs.len() > 1 {
        found.push(problem_in(
            file,
            "R072",
            "options.stav declares more than one struct, only `Options` belongs there",
        ));
    }
    sums.append(&mut own_sums);
}

/// Checks one sum type declaration and adds it.
fn read_sum(
    file: &str,
    text: &str,
    decl: TypeDecl<'_>,
    sums: &mut Vec<Sum>,
    found: &mut Vec<FileDiagnostic>,
) {
    let Some(name) = decl.name() else {
        return;
    };
    let name_text = name.text();
    if let Some(reserved) = reserved_type_name(name_text) {
        found.push(problem_at(
            file,
            text,
            name.span(),
            "R072",
            format!("the sum type `{name_text}` is named like {reserved}"),
        ));
        return;
    }
    let mut tags = Vec::new();
    for variant in decl.variants() {
        let Some(tag) = variant.name() else {
            continue;
        };
        let tag_text = tag.text();
        if tag_text == "true" || tag_text == "false" {
            found.push(problem_at(
                file,
                text,
                tag.span(),
                "R072",
                format!("the tag `{tag_text}` of `{name_text}` is spelled like a bool"),
            ));
            continue;
        }
        if !is_valid_name(tag_text) {
            found.push(problem_at(
                file,
                text,
                tag.span(),
                "R072",
                format!(
                    "the tag `{tag_text}` of `{name_text}` is not a name of lowercase letters, digits and `_`"
                ),
            ));
            continue;
        }
        if variant.params().is_some() {
            found.push(problem_at(
                file,
                text,
                tag.span(),
                "R072",
                format!(
                    "the tag `{tag_text}` of `{name_text}` carries a value, but an option is a bare tag"
                ),
            ));
            continue;
        }
        tags.push(tag_text.to_owned());
    }
    if sums.iter().any(|s| s.name == name_text) {
        found.push(problem_at(
            file,
            text,
            name.span(),
            "R072",
            format!("the sum type `{name_text}` is declared twice"),
        ));
        return;
    }
    sums.push(Sum {
        name: name_text.to_owned(),
        tags,
    });
}

/// Checks the fields of the `Options` struct and adds them.
fn read_fields(
    file: &str,
    text: &str,
    decl: StructDecl<'_>,
    sums: &mut Vec<&Sum>,
    fields: &mut Vec<Declaration>,
    found: &mut Vec<FileDiagnostic>,
) {
    let mut own: Vec<Declaration> = Vec::new();
    for field in decl.fields() {
        let Some(name) = field.name() else {
            continue;
        };
        let name_text = name.text();
        let at = |message: String| problem_at(file, text, name.span(), "R072", message);
        let Some(doc) = ast::doc_comment(field.syntax()) else {
            found.push(at(format!(
                "the option `{name_text}` has no doc comment saying what it does"
            )));
            continue;
        };
        let first = doc.lines().next().unwrap_or_default();
        if first.chars().count() > DOC_LINE_CHARS {
            found.push(at(format!(
                "the first line of the doc comment of `{name_text}` is longer than {DOC_LINE_CHARS} characters"
            )));
            continue;
        }
        let Some(kind) = field_kind(&field, sums, name_text, &at, found) else {
            continue;
        };
        let Some(default) = default_value(&field, &kind, sums, name_text, &at, found) else {
            continue;
        };
        if !is_valid_name(name_text) {
            found.push(at(format!(
                "the option name `{name_text}` is not a name of lowercase letters, digits and `_`"
            )));
            continue;
        }
        if own.iter().any(|f| f.name == name_text) {
            found.push(at(format!("the option `{name_text}` is declared twice")));
            continue;
        }
        own.push(Declaration {
            name: name_text.to_owned(),
            kind,
            default,
            doc: first.to_owned(),
        });
    }
    fields.append(&mut own);
}

/// The type of a field: `Bool` or a sum type of the same file.
fn field_kind(
    field: &Field<'_>,
    sums: &[&Sum],
    name: &str,
    at: &dyn Fn(String) -> FileDiagnostic,
    found: &mut Vec<FileDiagnostic>,
) -> Option<Kind> {
    let Type::Named(named) = field.ty()? else {
        found.push(at(format!(
            "the type of `{name}` is not `Bool` or a sum type of bare tags"
        )));
        return None;
    };
    if named.args().next().is_some() {
        found.push(at(
            "an option is `Bool` or a sum type of bare tags, without type arguments".to_owned(),
        ));
        return None;
    }
    let path = named.path()?;
    let segments: Vec<&str> = path.segments().map(|s| s.text()).collect();
    if segments.as_slice() == ["Bool"] {
        return Some(Kind::Bool);
    }
    if segments.len() == 1 && sums.iter().any(|s| s.name == segments[0]) {
        return Some(Kind::Sum(segments[0].to_owned()));
    }
    found.push(at(format!(
        "the type `{}` of `{name}` is neither `Bool` nor a sum type declared in options.stav",
        segments.join(".")
    )));
    None
}

/// The default of a field: `true`, `false`, or a bare tag of its sum type.
fn default_value(
    field: &Field<'_>,
    kind: &Kind,
    sums: &[&Sum],
    name: &str,
    at: &dyn Fn(String) -> FileDiagnostic,
    found: &mut Vec<FileDiagnostic>,
) -> Option<OptionValue> {
    let bad = |found: &mut Vec<FileDiagnostic>| {
        found.push(at(format!(
            "the option `{name}` has no default written as `true`, `false` or a bare tag of its type"
        )));
    };
    let Some(default) = field.default() else {
        bad(found);
        return None;
    };
    match (kind, default) {
        (Kind::Bool, Expr::Literal(literal)) => match literal.kind() {
            Some(LiteralKind::True) => Some(OptionValue::Bool(true)),
            Some(LiteralKind::False) => Some(OptionValue::Bool(false)),
            _ => {
                bad(found);
                None
            }
        },
        (Kind::Bool, _) => {
            bad(found);
            None
        }
        (Kind::Sum(sum), Expr::Name(tag)) => {
            let tag = tag.token().map(|t| t.text()).unwrap_or_default();
            let known = sums
                .iter()
                .find(|s| &s.name == sum)
                .is_some_and(|s| s.tags.iter().any(|t| t == tag));
            if known {
                Some(OptionValue::Tag(tag.to_owned()))
            } else {
                found.push(at(format!(
                    "the default `{tag}` of `{name}` is no tag of `{sum}`"
                )));
                None
            }
        }
        (Kind::Sum(_), _) => {
            bad(found);
            None
        }
    }
}

/// An error with `code` and `message` about the options of a package as
/// a whole, which no single file or place explains.
fn problem(code: &'static str, message: impl Into<String>) -> FileDiagnostic {
    FileDiagnostic {
        file: None,
        diagnostic: Diagnostic::error(code, Span { start: 0, end: 0 }, message),
    }
}

/// An error with `code` and `message` about the whole options `file`,
/// which names no place within it.
fn problem_in(file: &str, code: &'static str, message: impl Into<String>) -> FileDiagnostic {
    FileDiagnostic {
        file: None,
        diagnostic: Diagnostic::error(
            code,
            Span { start: 0, end: 0 },
            format!("{} ({file})", message.into()),
        ),
    }
}

/// An error with `code` and `message` about the options `file` at `span`
/// of `text`, which names the place in the message.
fn problem_at(
    file: &str,
    text: &str,
    span: Span,
    code: &'static str,
    message: impl Into<String>,
) -> FileDiagnostic {
    FileDiagnostic {
        file: None,
        diagnostic: Diagnostic::error(
            code,
            span,
            format!(
                "{} ({file}, {})",
                message.into(),
                position(text, span.start)
            ),
        ),
    }
}

/// The line and column of `at` bytes into `text`, the column counted in
/// characters.
fn position(text: &str, at: usize) -> String {
    let at = at.min(text.len());
    let before = &text[..at];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[start..].chars().count() + 1;
    format!("line {line}, column {column}")
}
