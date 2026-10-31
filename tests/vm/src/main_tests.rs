use super::*;

fn parse(args: &[&str]) -> Result<(bool, System)> {
    options(args.iter().map(|a| (*a).to_owned()).collect())
}

#[test]
fn default_and_base_modes_do_not_require_usable_packages() {
    assert_eq!(parse(&[]).unwrap(), (false, System::Minimal));
    assert_eq!(
        parse(&["--keep", "--self-built", "packages"]).unwrap(),
        (true, System::SelfBuilt("packages".into()))
    );
}

#[test]
fn usable_requires_an_explicit_package_directory_in_either_order() {
    for args in [
        vec!["--usable", "--self-built", "packages"],
        vec!["--self-built", "packages", "--usable"],
    ] {
        assert_eq!(
            parse(&args).unwrap(),
            (false, System::Usable("packages".into()))
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
fn usable_checks_install_and_publish_their_shell_tools() {
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
            system::USABLE_PACKAGES.contains(&tool) || system::SELF_BUILT_PACKAGES.contains(&tool),
            "missing published package: {tool}"
        );
        assert!(
            packages.split(',').any(|p| p.trim() == tool),
            "missing requested package: {tool}"
        );
    }
}

#[test]
fn command_labels_exclude_arguments_and_environment() {
    let mut command = Command::new("/private/directory/program");
    command
        .arg("private-argument")
        .env("PRIVATE", "private-value");
    assert_eq!(command_name(&command), "program");
}
