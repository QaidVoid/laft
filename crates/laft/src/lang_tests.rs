use super::*;

#[test]
fn options_reach_nested_structs_with_their_paths() {
    let options = options_of(&laft_config::schema().expect("the prelude checks"));
    let paths: Vec<&str> = options.iter().map(|o| o.path.as_str()).collect();
    assert!(paths.contains(&"hostname"), "{paths:?}");
    assert!(paths.contains(&"gc.keep_generations"), "{paths:?}");
    assert!(paths.contains(&"boot.initrd.shell"), "{paths:?}");
    assert!(
        paths.iter().any(|p| p.starts_with("repos.<name>.")),
        "{paths:?}"
    );
    let gc = options
        .iter()
        .find(|o| o.path == "gc.keep_days")
        .expect("listed");
    assert!(gc.ty.starts_with("Int"), "{}", gc.ty);
}

#[test]
fn a_reference_to_a_struct_is_an_option_of_its_own() {
    let options = options_of(&laft_config::schema().expect("the prelude checks"));
    let paths: Vec<&str> = options.iter().map(|o| o.path.as_str()).collect();
    assert!(paths.contains(&"users.accounts.<name>.group"), "{paths:?}");
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with("users.accounts.<name>.group.")),
        "{paths:?}"
    );
}

#[test]
fn stav_files_skip_secrets() {
    let dir = laft_test::tempdir();
    fs::create_dir_all(dir.path().join("secrets")).expect("writable");
    fs::create_dir_all(dir.path().join("parts")).expect("writable");
    for file in [
        "system.stav",
        "parts/net.stav",
        "secrets/key.stav",
        "notes.txt",
    ] {
        fs::write(dir.path().join(file), "").expect("writable");
    }
    let mut files = Vec::new();
    stav_files(dir.path(), &mut files).expect("readable");
    let names: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(dir.path())
                .expect("inside")
                .display()
                .to_string()
        })
        .collect();
    assert_eq!(names, ["parts/net.stav", "system.stav"]);
}
