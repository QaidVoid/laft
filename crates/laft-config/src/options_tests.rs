use std::collections::BTreeMap;

use laft_core::{Name, OptionValue};
use laft_index::{OptionDecl, OptionType};

use crate::{IndexOptions, options};

/// The declarations of mesa: one tag option and one bool option.
fn mesa() -> (Name, Vec<OptionDecl>) {
    (
        "mesa".parse::<Name>().expect("a name"),
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
    )
}

fn options_of(entries: Vec<(Name, Vec<OptionDecl>)>) -> IndexOptions {
    IndexOptions {
        options: entries.into_iter().collect(),
        profile: BTreeMap::new(),
    }
}

#[test]
fn renders_sum_types_structs_and_defaults() {
    let text = options::module(&options_of(vec![mesa()]));
    insta::assert_snapshot!(text, @r"
    # The package options of the enabled repositories, rendered from the
    # `options` members of their indexes. This module is generated on every
    # run and is not part of the configuration.

    ## The values of the option vulkan.
    type Mesa_Vulkan = all | amd | intel

    ## The options of mesa.
    struct Mesa_Options {
      ## Build the LLVM pipe.
      llvm:   Bool = true
      ## The Vulkan drivers.
      vulkan: Mesa_Vulkan = all
    }

    ## The package options, by package name.
    struct PackageOptions {
      ## The options of mesa.
      mesa: Mesa_Options = Mesa_Options {}
    }
    ");
}

#[test]
fn renders_an_empty_module_when_nothing_declares_options() {
    let text = options::module(&options_of(Vec::new()));
    insta::assert_snapshot!(text, @r"
    # The package options of the enabled repositories, rendered from the
    # `options` members of their indexes. This module is generated on every
    # run and is not part of the configuration.

    ## No package declares options.
    struct PackageOptions {}
    ");
}

#[test]
fn renders_the_names_of_packages_with_separators() {
    let name = "intel-media-driver".parse::<Name>().expect("a name");
    let decls = vec![OptionDecl {
        name: "va_api".to_owned(),
        kind: OptionType::Bool("bool".to_owned()),
        default: OptionValue::Bool(false),
        doc: String::new(),
    }];
    let text = options::module(&options_of(vec![(name, decls)]));
    assert!(text.contains("## The options of intel-media-driver."));
    assert!(text.contains("struct IntelMediaDriver_Options {"));
    assert!(text.contains("va_api: Bool = false"));
    assert!(text.contains("  intel-media-driver: IntelMediaDriver_Options"));
}

#[test]
fn leaves_out_names_that_cannot_be_fields() {
    let keyword = "in".parse::<Name>().expect("a name");
    let keyword_option = "pkg".parse::<Name>().expect("a name");
    let digit = "0ad".parse::<Name>().expect("a name");
    let decls = vec![OptionDecl {
        name: "type".to_owned(),
        kind: OptionType::Bool("bool".to_owned()),
        default: OptionValue::Bool(true),
        doc: "d".to_owned(),
    }];
    let kept = vec![OptionDecl {
        name: "nls".to_owned(),
        kind: OptionType::Bool("bool".to_owned()),
        default: OptionValue::Bool(true),
        doc: "d".to_owned(),
    }];
    let text = options::module(&options_of(vec![
        (keyword, kept.clone()),
        (keyword_option, decls.clone()),
        (digit, kept),
    ]));
    assert!(!text.contains(" in:"), "no field for the keyword: {text}");
    assert!(
        !text.contains(" 0ad"),
        "no field for a leading digit: {text}"
    );
    assert!(
        !text.contains("Pkg_Options"),
        "no struct for a package whose every option is a keyword: {text}"
    );
    assert!(text.contains("struct PackageOptions {"));
}

#[test]
fn keeps_the_first_line_of_a_multiline_doc_comment() {
    let name = "mesa".parse::<Name>().expect("a name");
    let decls = vec![OptionDecl {
        name: "llvm".to_owned(),
        kind: OptionType::Bool("bool".to_owned()),
        default: OptionValue::Bool(true),
        doc: "Build the LLVM pipe.\n\nSince 25.2.".to_owned(),
    }];
    let text = options::module(&options_of(vec![(name, decls)]));
    assert!(text.contains("## Build the LLVM pipe.\n"));
    assert!(!text.contains("Since 25.2"));
}

#[test]
fn renames_generated_types_that_would_collide() {
    let hyphen = "foo-bar".parse::<Name>().expect("a name");
    let underscore = "foo_bar".parse::<Name>().expect("a name");
    let decls = || {
        vec![OptionDecl {
            name: "baz".to_owned(),
            kind: OptionType::Tags(vec!["one".to_owned(), "two".to_owned()]),
            default: OptionValue::Tag("one".to_owned()),
            doc: "d".to_owned(),
        }]
    };
    let text = options::module(&options_of(vec![(hyphen, decls()), (underscore, decls())]));
    assert!(text.contains("type FooBar_Baz = one | two"));
    assert!(text.contains("type FooBar_2_Baz = one | two"));
    assert!(text.contains("struct FooBar_Options {"));
    assert!(text.contains("struct FooBar_2_Options {"));
    assert!(text.contains("foo-bar: FooBar_Options"));
    assert!(text.contains("foo_bar: FooBar_2_Options"));
}

#[test]
fn keeps_a_tagged_option_named_options() {
    let name = "mesa".parse::<Name>().expect("a name");
    let decls = vec![OptionDecl {
        name: "options".to_owned(),
        kind: OptionType::Tags(vec!["full".to_owned(), "slim".to_owned()]),
        default: OptionValue::Tag("full".to_owned()),
        doc: "d".to_owned(),
    }];
    let text = options::module(&options_of(vec![(name, decls)]));
    assert!(text.contains("struct Mesa_Options {"), "{text}");
    assert!(text.contains("type Mesa_Options_2 = full | slim"), "{text}");
    assert!(text.contains("options: Mesa_Options_2 = full"), "{text}");
}

#[test]
fn keeps_the_option_of_a_package_whose_other_option_is_a_keyword() {
    let name = "pkg".parse::<Name>().expect("a name");
    let decls = vec![
        OptionDecl {
            name: "type".to_owned(),
            kind: OptionType::Bool("bool".to_owned()),
            default: OptionValue::Bool(true),
            doc: "d".to_owned(),
        },
        OptionDecl {
            name: "nls".to_owned(),
            kind: OptionType::Bool("bool".to_owned()),
            default: OptionValue::Bool(true),
            doc: "d".to_owned(),
        },
    ];
    let text = options::module(&options_of(vec![(name, decls)]));
    assert!(text.contains("nls: Bool = true"));
    assert!(!text.contains("type:"));
}

#[test]
fn values_collect_fixed_preferred_and_profile_contributions() {
    let dir = laft_test::tempdir();
    std::fs::write(
        dir.path().join("part.stav"),
        "system {\n  hostname = \"forge\"\n  package { mesa { llvm = false } }\n}\n",
    )
    .expect("the file can be written");
    let (name, decls) = mesa();
    let mut options = IndexOptions {
        options: BTreeMap::from([(name.clone(), decls)]),
        profile: BTreeMap::new(),
    };
    options.profile.insert(
        name.clone(),
        [("vulkan".to_owned(), OptionValue::Tag("amd".to_owned()))]
            .into_iter()
            .collect(),
    );
    let evaluation = crate::evaluate(dir.path(), &dir.path().join("part.stav"), &options)
        .expect("the file evaluates");
    let values = options::values(
        &evaluation.checked,
        &evaluation.evaluated,
        &options,
        &BTreeMap::new(),
    );
    let system = &values[&"system".parse::<Name>().expect("a name")];
    assert_eq!(system.fixed[&name]["llvm"].value, OptionValue::Bool(false));
    assert_eq!(
        system.preferred[&name]["vulkan"],
        OptionValue::Tag("amd".to_owned()),
        "the profile of the index is preferred"
    );
    assert!(
        !system.preferred[&name].contains_key("llvm"),
        "a fixed value has no preferred value beside it"
    );
}

#[test]
fn values_prefer_nothing_a_fixed_value_already_holds() {
    let dir = laft_test::tempdir();
    std::fs::write(
        dir.path().join("part.stav"),
        "system {\n  hostname = \"forge\"\n  package.mesa.llvm = false\n}\n",
    )
    .expect("the file can be written");
    let (name, decls) = mesa();
    let mut options = IndexOptions {
        options: BTreeMap::from([(name.clone(), decls)]),
        profile: BTreeMap::new(),
    };
    options.profile.insert(
        name.clone(),
        [("llvm".to_owned(), OptionValue::Bool(false))]
            .into_iter()
            .collect(),
    );
    let evaluation = crate::evaluate(dir.path(), &dir.path().join("part.stav"), &options)
        .expect("the file evaluates");
    let values = options::values(
        &evaluation.checked,
        &evaluation.evaluated,
        &options,
        &BTreeMap::new(),
    );
    let system = &values[&"system".parse::<Name>().expect("a name")];
    assert_eq!(system.fixed[&name]["llvm"].value, OptionValue::Bool(false));
    assert!(
        !system.preferred[&name].contains_key("llvm"),
        "a fixed value has no preferred value beside it"
    );
}
