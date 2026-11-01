use super::*;

fn true_constraint(name: &str) -> OptionConstraint {
    OptionConstraint::True {
        name: name.to_owned(),
    }
}

fn false_constraint(name: &str) -> OptionConstraint {
    OptionConstraint::False {
        name: name.to_owned(),
    }
}

fn tag_constraint(name: &str, tag: &str) -> OptionConstraint {
    OptionConstraint::Tag {
        name: name.to_owned(),
        tag: tag.to_owned(),
    }
}

#[test]
fn parses_every_constraint_form() {
    assert_eq!(
        "llvm".parse::<OptionConstraint>().unwrap(),
        true_constraint("llvm")
    );
    assert_eq!(
        "!x11".parse::<OptionConstraint>().unwrap(),
        false_constraint("x11")
    );
    assert_eq!(
        "vulkan=amd".parse::<OptionConstraint>().unwrap(),
        tag_constraint("vulkan", "amd")
    );
}

#[test]
fn rejects_constraints_that_are_not_names() {
    for text in [
        "",
        "!",
        "!x11=false",
        "X11",
        "x11=amd,x11=intel",
        "x11=true",
        "x11=false",
        "true",
        "!false",
    ] {
        assert!(
            text.parse::<OptionConstraint>().is_err(),
            "`{text}` should not parse"
        );
    }
}

#[test]
fn constraints_match_their_own_value_only() {
    let values = [
        ("llvm", OptionValue::Bool(true)),
        ("x11", OptionValue::Bool(false)),
        ("vulkan", OptionValue::Tag("amd".into())),
        ("vulkan", OptionValue::Tag("intel".into())),
    ];
    let expects = [
        (true_constraint("llvm"), "llvm"),
        (false_constraint("x11"), "x11"),
        (tag_constraint("vulkan", "amd"), "vulkan"),
    ];
    for (constraint, option) in &expects {
        for (name, value) in &values {
            let matches = name == option && constraint.matches(value);
            assert_eq!(
                constraint.matches(value),
                matches,
                "{constraint:?} against {name}={value:?}"
            );
        }
    }
}

#[test]
fn constraints_display_their_text() {
    let constraints = vec![
        true_constraint("llvm"),
        false_constraint("x11"),
        tag_constraint("vulkan", "amd"),
    ];
    let all: OptionConstraints = constraints.clone().into();
    assert_eq!(all.to_string(), "{llvm,!x11,vulkan=amd}");
    assert_eq!(all.as_slice(), constraints);
    assert!(!all.is_empty());
    assert_eq!(OptionConstraints::default().to_string(), "");
    assert!(OptionConstraints::default().is_empty());
}

#[test]
fn parses_a_constraint_list_with_its_braces() {
    let list: OptionConstraints = "{llvm,!x11,vulkan=amd}".parse().unwrap();
    assert_eq!(
        list.as_slice(),
        &[
            true_constraint("llvm"),
            false_constraint("x11"),
            tag_constraint("vulkan", "amd")
        ]
    );
}

#[test]
fn rejects_broken_constraint_lists() {
    for text in [
        "",
        "{}",
        "{",
        "}",
        "llvm",
        "{llvm,}",
        "{,llvm}",
        "{ llvm }",
        "{x11=true}",
        "{llvm,llvm}",
        "{llvm,!llvm}",
        "{vulkan=amd,vulkan=intel}",
        "{llvm,vulkan=amd,llvm}",
    ] {
        assert!(
            text.parse::<OptionConstraints>().is_err(),
            "`{text}` should not parse"
        );
    }
    assert_eq!(
        "llvm".parse::<OptionConstraints>().unwrap_err().reason(),
        "expected `{flag,...}` with at least one constraint and no spaces"
    );
    assert_eq!(
        "{llvm,llvm}"
            .parse::<OptionConstraints>()
            .unwrap_err()
            .reason(),
        "each option may be constrained at most once"
    );
}
