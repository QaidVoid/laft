use super::*;

fn request(text: &str, origin: &str) -> Request {
    Request {
        pkg: text.parse().expect("valid reference"),
        origin: origin.into(),
    }
}

fn texts(requests: &[Request]) -> Vec<String> {
    requests.iter().map(|r| r.pkg.to_string()).collect()
}

fn config() -> SystemConfig {
    let dir = laft_test::tempdir();
    std::fs::write(
        dir.path().join("system.stav"),
        "system {\n  packages = [ripgrep, llvm]\n  alternatives.vi = neovim\n  \
         fs \"/\" { device = \"UUID=1\", fstype = ext4 }\n  \
         fs \"/boot\" { device = \"UUID=2\", fstype = vfat }\n  \
         view legacy { packages = [llvm@19, openssl] }\n  \
         view bare { inherit = none, packages = [cmake] }\n}\n",
    )
    .expect("writable");
    laft_config::load(dir.path(), &|_| Ok(()))
        .expect("valid")
        .config
}

#[test]
fn a_view_that_inherits_replaces_system_requests_of_the_same_name() {
    let config = config();
    let system = vec![request("ripgrep", "a"), request("llvm", "b")];
    let (requests, alternatives) =
        view_requests(&config, &"legacy".parse().expect("valid"), &system);
    assert_eq!(texts(&requests), ["ripgrep", "llvm@19", "openssl"]);
    assert_eq!(
        alternatives
            .get(&"vi".parse().expect("valid"))
            .map(Name::as_str),
        Some("neovim")
    );
    let (requests, alternatives) = view_requests(&config, &"bare".parse().expect("valid"), &system);
    assert_eq!(texts(&requests), ["cmake"]);
    assert!(alternatives.is_empty());
}

#[test]
fn the_system_requests_add_the_kernel_and_service_packages_once() {
    let config = config();
    let services = services(&config).expect("valid services");
    let requests = system_requests(&config, &services);
    assert_eq!(texts(&requests)[..3], ["llvm", "ripgrep", "linux"]);
    let kernel = requests.iter().find(|r| r.pkg.name().as_str() == "linux");
    assert_eq!(kernel.map(|r| r.origin.as_str()), Some("boot.kernel"));
    let names: Vec<&str> = requests.iter().map(|r| r.pkg.name().as_str()).collect();
    let unique: std::collections::BTreeSet<&&str> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
}
