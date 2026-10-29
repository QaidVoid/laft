use super::*;

#[test]
fn the_first_line_is_the_message_and_the_rest_is_context() {
    let failure = Failure::general("view \"system\": 1 path conflict\n  usr/bin/vi\n");
    assert_eq!(failure.message, "view \"system\": 1 path conflict");
    assert_eq!(failure.context, ["  usr/bin/vi"]);
    assert_eq!(failure.exit, exit::FAILURE);
}

#[test]
fn alternatives_errors_are_conflicts_and_other_resolution_errors_are_not() {
    let alternative = laft_resolve::Error {
        view: "system".into(),
        kind: Box::new(laft_resolve::ErrorKind::AlternativeAmbiguous {
            group: "vi".parse().expect("valid"),
            members: vec![
                "neovim".parse().expect("valid"),
                "vim".parse().expect("valid"),
            ],
        }),
    };
    assert_eq!(Failure::from(alternative).exit, exit::CONFLICT);
    let missing = laft_resolve::Error {
        view: "system".into(),
        kind: Box::new(laft_resolve::ErrorKind::NoCandidate {
            request: "htpo".into(),
            origin: "/etc/laft/system.stav:3:14".into(),
            available: vec![],
            replaced_by: vec![],
        }),
    };
    let failure = Failure::from(missing);
    assert_eq!(failure.exit, exit::RESOLVE);
    assert_eq!(failure.code.as_deref(), Some("R001"));
}

#[test]
fn a_busy_lock_and_path_conflicts_keep_their_meaning() {
    let conflicts = laft_store::Error::Conflicts {
        view: "system".into(),
        conflicts: vec![laft_store::Conflict {
            path: "usr/bin/vi".into(),
            owners: vec!["neovim 0.11.4-r1".into(), "vim 9.1-r1".into()],
        }],
    };
    let failure = Failure::from(conflicts);
    assert_eq!(failure.exit, exit::CONFLICT);
    assert!(failure.context.iter().any(|c| c.contains("usr/bin/vi")));
}
