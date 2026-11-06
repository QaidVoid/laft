use super::*;

fn parse(args: &[&str]) -> Result<(bool, System, bool)> {
    options(args.iter().map(|a| (*a).to_owned()).collect())
}

#[test]
fn default_and_base_modes_do_not_require_usable_packages() {
    assert_eq!(parse(&[]).unwrap(), (false, System::Minimal, false));
    assert_eq!(
        parse(&["--keep", "--self-built", "packages"]).unwrap(),
        (true, System::SelfBuilt("packages".into()), false)
    );
}

#[test]
fn the_options_mode_runs_alone() {
    assert_eq!(
        parse(&["--options"]).unwrap(),
        (false, System::Minimal, true)
    );
    assert!(parse(&["--options", "--usable"]).is_err());
    assert!(parse(&["--options", "--self-built", "packages"]).is_err());
}

#[test]
fn usable_requires_an_explicit_package_directory_in_either_order() {
    for args in [
        vec!["--usable", "--self-built", "packages"],
        vec!["--self-built", "packages", "--usable"],
    ] {
        assert_eq!(
            parse(&args).unwrap(),
            (false, System::Usable("packages".into()), false)
        );
    }
    for args in [
        vec!["--usable"],
        vec!["--self-built"],
        vec!["--self-built", "--usable"],
        vec!["--self-built", "one", "--self-built", "two"],
        vec!["--unknown"],
    ] {
        assert!(parse(&args).is_err());
    }
}

#[test]
fn usable_system_requests_and_publishes_its_test_tools() {
    let config = include_str!("../system-usable.stav");
    let packages = config
        .split("packages = [")
        .nth(1)
        .expect("package list")
        .split(']')
        .next()
        .expect("package entries");
    for tool in ["findutils", "diffutils"] {
        assert!(
            system::USABLE_PACKAGES.contains(&tool),
            "missing published package: {tool}"
        );
        assert!(
            packages.split(',').any(|p| p.trim() == tool),
            "missing requested package: {tool}"
        );
    }
}

#[test]
fn installer_errors_include_failure_after_package_installation() {
    let transcript = format!("{}1\n", installer::DONE);
    let error = check_install_result(&transcript, Path::new("install.log")).unwrap_err();
    assert!(
        error.starts_with("installer exited with status 1"),
        "{error}"
    );
    assert!(!error.contains("laft install exited"));
    assert!(
        check_install_result(&format!("{}0\n", installer::DONE), Path::new("install.log")).is_ok()
    );
}

#[test]
fn command_labels_exclude_arguments_and_environment() {
    let mut command = Command::new("/private/directory/program");
    command
        .arg("private-argument")
        .env("PRIVATE", "private-value");
    assert_eq!(command_name(&command), "program");
}

/// The stubs must be indexable: their buildinfo files satisfy CI003, and
/// their alternative groups pass the KR004 checks without a configured
/// default.
#[test]
fn the_stubs_pass_the_repository_checks() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("vmtest-stubs-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let stubs = dir.join("stubs");
    fs::create_dir_all(&stubs).map_err(|e| fail(format!("{}: {e}", stubs.display())))?;
    packages::build_view_stubs(&stubs)?;
    let key = dir.join("online");
    let repo = dir.join("repo");
    fs::create_dir_all(&repo).map_err(|e| fail(format!("{}: {e}", repo.display())))?;
    let tool = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the manifest directory has a parent")
        .join("../target/debug/laft-repo");
    let mut command = Command::new(&tool);
    command.args(["keygen", "--unencrypted", "--out"]).arg(&key);
    run(&mut command)?;
    let recipes = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes");
    let mut command = Command::new(&tool);
    command
        .args(["index", "--repo"])
        .arg(&repo)
        .args([
            "--name",
            "core",
            "--channel",
            "current",
            "--allow-bootstrap",
            "--key",
        ])
        .arg(key.with_extension("key"))
        .arg("--recipes")
        .arg(&recipes)
        .arg(&stubs);
    let result = run(&mut command);
    let _ = fs::remove_dir_all(&dir);
    result
}
