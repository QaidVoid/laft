//! Configurations shared by the unit tests.

use std::collections::BTreeMap;

use crate::{Dir, Limits, LogRotation, Ready, Restart, Service, ServiceConfig, ServiceSet};

/// The services the base part of the system configuration declares, which
/// a test of the configuration renders from the base part itself.
pub(crate) fn standard_services() -> BTreeMap<String, Service> {
    serde_json::from_str(include_str!("../tests/fixtures/standard-services.json"))
        .expect("the standard services fixture deserializes")
}

/// The standard services plus `extra`, with the default gettys.
pub(crate) fn config(extra: &[(&str, Service)]) -> ServiceConfig {
    let mut services = standard_services();
    for (name, service) in extra {
        services.insert((*name).to_owned(), service.clone());
    }
    ServiceConfig {
        services,
        ..ServiceConfig::default()
    }
}

/// The D-Bus system bus, as the example configuration declares it.
pub(crate) fn dbus() -> Service {
    Service {
        description: "D-Bus system message bus".into(),
        ready: Ready::Notify { fd: 3 },
        runtime_dirs: vec![Dir {
            owner: Some("messagebus".into()),
            ..Dir::new("dbus")
        }],
        ..Service::longrun(&[
            "/usr/bin/dbus-daemon",
            "--system",
            "--nofork",
            "--nopidfile",
            "--print-address=3",
        ])
    }
}

/// An application that waits for D-Bus, as the example configuration
/// declares it.
pub(crate) fn myapp() -> Service {
    Service {
        user: "myapp".into(),
        env: BTreeMap::from([("RUST_LOG".into(), "info".into())]),
        cwd: "/var/lib/myapp".into(),
        after: vec!["dbus".into()],
        state_dirs: vec![Dir {
            mode: 0o750,
            ..Dir::new("myapp")
        }],
        restart: Restart::OnFailure,
        limits: Limits {
            nofile: Some(65536),
            ..Limits::default()
        },
        rotate: LogRotation {
            size: 5_000_000,
            count: 5,
        },
        ..Service::longrun(&["/usr/bin/myapp", "--listen", "127.0.0.1:8080"])
    }
}

/// The example configuration: the standard services, `dbus` and `myapp`.
pub(crate) fn example() -> ServiceConfig {
    config(&[("dbus", dbus()), ("myapp", myapp())])
}

/// Validates `config`, failing the test with every error when it is
/// invalid.
pub(crate) fn valid(config: &ServiceConfig) -> ServiceSet {
    ServiceSet::new(config).unwrap_or_else(|errors| {
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("the configuration is invalid:\n{}", messages.join("\n"))
    })
}

/// The error messages for `config`, which must be invalid.
pub(crate) fn errors(config: &ServiceConfig) -> Vec<String> {
    match ServiceSet::new(config) {
        Ok(_) => panic!("the configuration is valid"),
        Err(errors) => errors.iter().map(ToString::to_string).collect(),
    }
}
