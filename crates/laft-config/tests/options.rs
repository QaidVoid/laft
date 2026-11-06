//! The generated `PackageOptions` module: what it type checks, and what
//! a load records about it.

mod support;

use std::collections::BTreeMap;

use laft_config::IndexOptions;
use laft_core::{Name, OptionValue};
use laft_index::{OptionDecl, OptionType};
use support::{DISKS, config_dir};

/// The declarations of mesa, as an index records them.
fn mesa_options() -> IndexOptions {
    let name = "mesa".parse::<Name>().expect("a name");
    IndexOptions {
        options: [(
            name,
            vec![
                OptionDecl {
                    name: "llvm".to_owned(),
                    kind: OptionType::Bool("bool".to_owned()),
                    default: OptionValue::Bool(true),
                    doc: "Build the LLVM pipe.".to_owned(),
                },
                OptionDecl {
                    name: "vulkan".to_owned(),
                    kind: OptionType::Tags(vec![
                        "all".to_owned(),
                        "amd".to_owned(),
                        "intel".to_owned(),
                    ]),
                    default: OptionValue::Tag("all".to_owned()),
                    doc: "The Vulkan drivers.".to_owned(),
                },
            ],
        )]
        .into_iter()
        .collect(),
        profile: BTreeMap::new(),
    }
}

/// Writes a configuration whose body is `body` and loads it with the
/// declarations of `options`.
fn load(body: &str, options: &IndexOptions) -> Result<laft_config::Loaded, laft_config::Report> {
    let dir = config_dir(&[("system.stav", &format!("system {{\n{DISKS}\n{body}\n}}\n"))]);
    support::load_with(dir.path(), options)
}

#[test]
fn a_typo_in_an_option_name_suggests_the_declared_one() {
    let report = load("package.mesa.llvn = false", &mesa_options()).unwrap_err();
    assert_eq!(support::codes(&report), ["E0302"]);
    let error = report.iter().next().expect("one diagnostic");
    assert!(
        error.diagnostic.message.contains("has no field `llvn`"),
        "{}",
        error.diagnostic.message
    );
    assert_eq!(
        error.diagnostic.help.as_deref(),
        Some("did you mean `llvm`?")
    );
}

#[test]
fn a_typo_in_a_bool_value_suggests_the_literal() {
    let report = load("package.mesa.llvm = flase", &mesa_options()).unwrap_err();
    assert_eq!(support::codes(&report), ["E0302"]);
    assert_eq!(
        report.iter().next().unwrap().diagnostic.help.as_deref(),
        Some("did you mean `false`?")
    );
}

#[test]
fn a_tag_the_option_does_not_declare_is_a_type_error() {
    let report = load("package.mesa.vulkan = nvidia", &mesa_options()).unwrap_err();
    let message = report.iter().next().unwrap().diagnostic.message.clone();
    assert!(
        message.contains("which is not a variant of `Mesa_Vulkan`"),
        "{message}"
    );
}

#[test]
fn an_option_no_repository_declares_is_an_unknown_field() {
    let report = load("package.mesa.llvm = false", &support::no_options()).unwrap_err();
    assert_eq!(support::codes(&report), ["E0302"]);
    let message = report.iter().next().unwrap().diagnostic.message.clone();
    assert!(message.contains("has no field `mesa`"), "{message}");
}

#[test]
fn declared_options_type_check_and_take_their_defaults() {
    let loaded =
        load("package { mesa { vulkan = amd } }", &mesa_options()).expect("the options check");
    assert!(loaded.warnings.is_empty());
}

#[test]
fn a_view_can_set_its_own_options() {
    let body = "view web { groups = [], package.mesa.llvm = false }";
    let loaded = load(body, &mesa_options()).expect("the view checks");
    assert!(
        loaded
            .config
            .views
            .contains_key(&"web".parse().expect("a name"))
    );
}

#[test]
fn the_generated_module_is_recorded_and_not_a_file() {
    let loaded = load("package.mesa.llvm = false", &mesa_options()).expect("loads");
    assert!(
        loaded
            .package_module
            .contains("type Mesa_Vulkan = all | amd | intel")
    );
    assert!(loaded.package_module.contains("struct Mesa_Options {"));
    assert!(loaded.package_module.contains("llvm:   Bool = true"));
    assert!(
        !loaded
            .files
            .iter()
            .any(|f| f.path.contains("package-options")),
        "the generated module is not a file of the configuration: {:?}",
        loaded.files
    );
}

#[test]
fn the_schema_describes_the_generated_options() {
    let schema = laft_config::schema(&mesa_options()).expect("the prelude checks");
    let package = schema
        .structs
        .iter()
        .find(|s| s.name == "PackageOptions")
        .expect("the generated struct is in the schema");
    assert_eq!(package.fields.len(), 1);
    assert_eq!(package.fields[0].name, "mesa");
    assert_eq!(package.fields[0].ty, "Mesa_Options");
    let options = schema
        .structs
        .iter()
        .find(|s| s.name == "Mesa_Options")
        .expect("the options struct is in the schema");
    assert_eq!(
        options
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["llvm", "vulkan"]
    );
    assert_eq!(
        options.fields[0].doc.as_deref(),
        Some("Build the LLVM pipe.")
    );
    assert!(schema.sums.iter().any(|s| s.name == "Mesa_Vulkan"));
}

#[test]
fn unset_options_prefer_their_declared_defaults() {
    let loaded = load("", &mesa_options()).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    assert!(values.fixed.is_empty());
    assert_eq!(
        values.preferred[&"mesa".parse::<Name>().expect("a name")]["llvm"],
        OptionValue::Bool(true)
    );
    assert_eq!(
        values.preferred[&"mesa".parse::<Name>().expect("a name")]["vulkan"],
        OptionValue::Tag("all".to_owned())
    );
}

#[test]
fn plain_and_forced_values_are_fixed_and_defaults_are_preferred() {
    let body = "package { mesa { llvm = false, vulkan = default(amd) } }";
    let loaded = load(body, &mesa_options()).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    let mesa = &"mesa".parse::<Name>().expect("a name");
    assert_eq!(values.fixed[mesa]["llvm"].value, OptionValue::Bool(false));
    assert_eq!(
        values.preferred[mesa]["vulkan"],
        OptionValue::Tag("amd".to_owned())
    );
    let body = "package.mesa.llvm = force(false)";
    let loaded = load(body, &mesa_options()).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    assert_eq!(
        values.fixed[&"mesa".parse::<Name>().expect("a name")]["llvm"].value,
        OptionValue::Bool(false)
    );
}

#[test]
fn profile_values_are_preferred_and_lose_to_the_configuration() {
    let mut options = mesa_options();
    options.profile.insert(
        "mesa".parse::<Name>().expect("a name"),
        [("llvm".to_owned(), OptionValue::Bool(false))]
            .into_iter()
            .collect(),
    );
    let loaded = load("", &options).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    let mesa = &"mesa".parse::<Name>().expect("a name");
    assert_eq!(values.preferred[mesa]["llvm"], OptionValue::Bool(false));
    assert_eq!(
        values.preferred[mesa]["vulkan"],
        OptionValue::Tag("all".to_owned())
    );
    let loaded = load("package.mesa.llvm = default(true)", &options).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    assert_eq!(values.preferred[mesa]["llvm"], OptionValue::Bool(true));
    let loaded = load("package.mesa.llvm = false", &options).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    assert_eq!(values.fixed[mesa]["llvm"].value, OptionValue::Bool(false));
    assert_eq!(
        values.preferred[mesa]["llvm"],
        OptionValue::Bool(false),
        "the profile's value stays preferred beside the fixed value"
    );
}

#[test]
fn every_view_resolves_from_the_system_and_merges_over_it() {
    let mut options = mesa_options();
    options.profile.insert(
        "mesa".parse::<Name>().expect("a name"),
        [("llvm".to_owned(), OptionValue::Bool(false))]
            .into_iter()
            .collect(),
    );
    let body = concat!(
        "package.mesa.vulkan = amd\n",
        "view web { package.mesa.vulkan = default(intel), package.mesa.llvm = false }\n",
        "view bare { inherit = none }\n",
    );
    let loaded = load(body, &options).expect("loads");
    let mesa = &"mesa".parse::<Name>().expect("a name");
    let web = &loaded.options[&"web".parse::<Name>().expect("a name")];
    assert_eq!(web.fixed[mesa]["llvm"].value, OptionValue::Bool(false));
    assert_eq!(
        web.fixed[mesa]["vulkan"].value,
        OptionValue::Tag("amd".to_owned())
    );
    assert_eq!(
        web.preferred[mesa]["vulkan"],
        OptionValue::Tag("all".to_owned()),
        "the declared default stays preferred beside the fixed value"
    );
    let bare = &loaded.options[&"bare".parse::<Name>().expect("a name")];
    assert_eq!(
        bare.fixed[mesa]["vulkan"].value,
        OptionValue::Tag("amd".to_owned()),
        "inherit = none governs package requests, not option values"
    );
    assert_eq!(bare.preferred[mesa]["llvm"], OptionValue::Bool(false));
    assert_eq!(
        bare.preferred[mesa]["vulkan"],
        OptionValue::Tag("all".to_owned()),
        "the declared default stays preferred beside the fixed value"
    );
    let system = &loaded.options[&"system".parse::<Name>().expect("a name")];
    assert_eq!(
        system.fixed[mesa]["vulkan"].value,
        OptionValue::Tag("amd".to_owned())
    );
    assert_eq!(system.preferred[mesa]["llvm"], OptionValue::Bool(false));
}

#[test]
fn a_view_default_does_not_lift_a_fixed_value_of_the_system() {
    let body = concat!(
        "package.mesa.llvm = false\n",
        "view web { package.mesa.vulkan = default(intel) }\n",
    );
    let loaded = load(body, &mesa_options()).expect("loads");
    let mesa = &"mesa".parse::<Name>().expect("a name");
    let web = &loaded.options[&"web".parse::<Name>().expect("a name")];
    assert_eq!(web.fixed[mesa]["llvm"].value, OptionValue::Bool(false));
    assert_eq!(
        web.preferred[mesa]["vulkan"],
        OptionValue::Tag("intel".to_owned()),
        "the view's default holds for the options nothing fixes"
    );
    assert_eq!(
        web.preferred[mesa]["llvm"],
        OptionValue::Bool(true),
        "the declared default stays preferred beside the fixed value"
    );
}

#[test]
fn the_schema_without_options_has_no_package_field() {
    let schema = laft_config::schema(&support::no_options()).expect("the prelude checks");
    let package = schema
        .structs
        .iter()
        .find(|s| s.name == "PackageOptions")
        .expect("the empty struct is in the schema");
    assert!(package.fields.is_empty());
}

#[test]
fn a_package_whose_option_is_named_options_still_type_checks() {
    let mesa = "mesa".parse::<Name>().expect("a name");
    let options = IndexOptions {
        options: [(
            mesa.clone(),
            vec![OptionDecl {
                name: "options".to_owned(),
                kind: OptionType::Tags(vec!["full".to_owned(), "slim".to_owned()]),
                default: OptionValue::Tag("full".to_owned()),
                doc: "The package shape.".to_owned(),
            }],
        )]
        .into_iter()
        .collect(),
        profile: BTreeMap::new(),
    };
    let loaded = load("package.mesa.options = slim", &options).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    assert_eq!(
        values.fixed[&mesa]["options"].value,
        OptionValue::Tag("slim".to_owned())
    );
}

#[test]
fn the_repositories_are_readable_when_the_options_do_not_type_check() {
    let body = concat!(
        "repo core { url = \"https://example.invalid/core\", ",
        "key = \"RWSO9Zvh1ju0E6MO5PDnzQFYnptNxsLpLdwYSfm5Cs4Jc+14umRuka1M\" }\n",
        "package.mesa.vulkan = amd\n",
    );
    let dir = config_dir(&[("system.stav", &format!("system {{\n{DISKS}\n{body}\n}}\n"))]);
    assert!(laft_config::load(dir.path(), &|_| Ok(()), &support::no_options()).is_err());
    let repos = laft_config::repos(dir.path()).expect("the repositories are read");
    let core = repos[&"core".parse::<Name>().expect("a name")].clone();
    assert_eq!(core.url, "https://example.invalid/core");
    assert_eq!(core.channel.to_string(), "current");
}

#[test]
fn an_option_the_module_leaves_out_is_reported() {
    let mesa = "mesa".parse::<Name>().expect("a name");
    let options = IndexOptions {
        options: [(
            mesa,
            vec![
                OptionDecl {
                    name: "type".to_owned(),
                    kind: OptionType::Bool("bool".to_owned()),
                    default: OptionValue::Bool(true),
                    doc: "d".to_owned(),
                },
                OptionDecl {
                    name: "flavor".to_owned(),
                    kind: OptionType::Tags(vec!["one".to_owned(), "none".to_owned()]),
                    default: OptionValue::Tag("one".to_owned()),
                    doc: "d".to_owned(),
                },
                OptionDecl {
                    name: "keep".to_owned(),
                    kind: OptionType::Bool("bool".to_owned()),
                    default: OptionValue::Bool(true),
                    doc: "d".to_owned(),
                },
            ],
        )]
        .into_iter()
        .collect(),
        profile: BTreeMap::new(),
    };
    let loaded = load("package.mesa.keep = false", &options).expect("loads");
    let notes: Vec<String> = loaded
        .warnings
        .iter()
        .filter(|d| d.diagnostic.code == "C0905")
        .map(|d| d.diagnostic.message.clone())
        .collect();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(
        notes[0].contains("the option type of package mesa is left out: type is a Stav keyword"),
        "{notes:?}"
    );
    assert!(
        notes[1].contains(
            "the option flavor of package mesa is left out: the tag none cannot be a variant"
        ),
        "{notes:?}"
    );
}

#[test]
fn fixed_values_remember_where_the_configuration_set_them() {
    let loaded = load("package { mesa { llvm = false } }", &mesa_options()).expect("loads");
    let values = &loaded.options[&"system".parse::<Name>().expect("a name")];
    let llvm = &values.fixed[&"mesa".parse::<Name>().expect("a name")]["llvm"];
    assert_eq!(llvm.value, OptionValue::Bool(false));
    assert!(llvm.origin.contains("system.stav"), "{}", llvm.origin);
    let (rest, col) = llvm.origin.rsplit_once(':').expect("a column");
    let (_, line) = rest.rsplit_once(':').expect("a line");
    assert!(line.parse::<usize>().is_ok(), "{}", llvm.origin);
    assert!(line != "0" && !col.is_empty(), "{}", llvm.origin);
    let parsed: usize = col.parse().expect("a numeric column");
    assert!(parsed >= 1, "{}", llvm.origin);
}
