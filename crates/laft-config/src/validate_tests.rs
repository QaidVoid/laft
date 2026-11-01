use super::*;

#[test]
fn missing_immutable_root_points_at_the_mutable_assignment() {
    let dir = laft_test::tempdir();
    let source = "system { users { mutable = false } }\n";
    std::fs::write(dir.path().join("system.stav"), source).expect("can write configuration");
    let empty = crate::options::module(&crate::IndexOptions::default());
    let mut host = crate::host::host(dir.path(), &empty);
    host.implicit_parts.clear();
    let root = stav::RootSpec {
        root_struct: "Root".to_owned(),
        entries: vec![dir.path().join("system.stav")],
        import_roots: vec![stav::ImportRoot::new(dir.path())],
    };
    let checked = stav::check(&host, &root).expect("the configuration type checks");
    let evaluated = checked
        .eval(&stav::Limits::default())
        .expect("the configuration evaluates");
    let cx = Cx::new(&checked, &evaluated);
    let mut found = Vec::new();
    let etc = convert::etc(&cx, &mut found).expect("account settings convert");
    assert!(found.is_empty());
    let errors = check_config(&etc, &laft_activate::Security::default());
    assert_eq!(errors.len(), 1, "{errors:?}");
    let diagnostic = etc_error(&cx, &errors[0]).expect("the error has a diagnostic");
    assert_eq!(diagnostic.diagnostic.code, "C0416");
    assert_eq!(&source[diagnostic.diagnostic.span.range()], "false");
    assert!(diagnostic.diagnostic.message.contains("root"));
    assert!(
        diagnostic
            .diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("user root"))
    );
}
