//! Activation of a running system: the generation is mounted below
//! `/run/laft/mounts`, `/run/laft/system` is switched to it with one
//! rename, and the views, `/etc`, the accounts and the services follow.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use laft_activate::OnConflict;
use laft_store::Lock;

use crate::activate::Activation;
use crate::env::Env;
use crate::failure::Failure;
use crate::worker::{self, Request};

/// Points the symlink `link` at `target` with one rename.
fn replace_link(link: &Path, target: &str) -> std::io::Result<()> {
    let tmp = link.with_file_name(format!(
        "{}.new",
        link.file_name().and_then(|n| n.to_str()).unwrap_or("link")
    ));
    let _ = fs::remove_file(&tmp);
    symlink(target, &tmp)?;
    fs::rename(&tmp, link)
}

/// Switches the running system to generation `number`.
///
/// Conflicts in `/etc` stop the switch before anything changes. The
/// generation is then mounted, and renaming `/run/laft/system` is the
/// moment the running system uses it. After that, every remaining step
/// runs even when one fails, and failures are reported in the result.
pub(crate) fn activate(
    env: &Env,
    lock: &Lock,
    number: u64,
    adopt: &BTreeSet<String>,
) -> Result<Activation, Failure> {
    let conflicts = laft_activate::check_etc(lock, number, adopt)?;
    if !conflicts.is_empty() {
        return Err(laft_activate::Error::Conflicts(conflicts).into());
    }
    let generation = env.generation(number)?;
    let previous = env.active()?;
    let mut activation = Activation::default();

    worker::request(env, &Request::Mount { generation: number })?;
    activation.done.push("mount");
    replace_link(&env.path("/run/laft/system"), &format!("mounts/{number}"))
        .map_err(|e| Failure::general(format!("switching /run/laft/system: {e}")))?;
    activation.done.push("switch");
    activation.active = true;

    let mut views = Ok(());
    for (name, version) in &generation.views {
        let link = env.path(&format!("/laft/views/{name}/current"));
        if let Err(e) = replace_link(&link, &version.to_string()) {
            views = Err(format!("{}: {e}", link.display()));
        }
    }
    record(&mut activation, "views", views);

    let etc = laft_activate::reconcile_etc(lock, number, adopt, OnConflict::Stop);
    if let Ok(report) = &etc {
        for warning in &report.warnings {
            env.ui.warn(warning.to_string());
        }
    }
    record(
        &mut activation,
        "etc",
        etc.map(|_| ()).map_err(|e| e.to_string()),
    );
    let accounts = laft_activate::apply_accounts(lock, number);
    if let Ok(report) = &accounts {
        for warning in &report.warnings {
            env.ui.warn(warning.to_string());
        }
    }
    record(
        &mut activation,
        "accounts",
        accounts.map(|_| ()).map_err(|e| e.to_string()),
    );
    activation.done.push("entry-links");

    services(env, &mut activation, previous, number);

    let booted = env.booted()?;
    let keep: Vec<u64> = std::iter::once(number).chain(booted).collect();
    let unmounted = worker::request(env, &Request::Unmount { keep }).map_err(|f| f.message);
    record(&mut activation, "unmount", unmounted);
    Ok(activation)
}

fn record(activation: &mut Activation, step: &'static str, result: Result<(), String>) {
    match result {
        Ok(()) => activation.done.push(step),
        Err(why) => activation.failed.push((step, why)),
    }
}

/// Brings the running services in line with generation `number`, when the
/// running init is the generation's.
fn services(env: &Env, activation: &mut Activation, previous: Option<u64>, number: u64) {
    let backend = fs::read_to_string(env.path("/run/laft/init-backend")).unwrap_or_default();
    let configured = env
        .root
        .generation(number)
        .map(|g| g.init.backend)
        .unwrap_or_default();
    if backend.trim() != configured {
        env.ui.note(format!(
            "services    skipped because the running init is not {configured}. Reboot to use it."
        ));
        return;
    }
    let units =
        |n: u64| laft_svc::runit::load_units(&env.path(&format!("/laft/gens/{n}/services")));
    let old = previous.map_or_else(|| Ok(Default::default()), units);
    let (old, new) = match (old, units(number)) {
        (Ok(old), Ok(new)) => (old, new),
        (Err(e), _) | (_, Err(e)) => {
            activation.failed.push(("services", e.to_string()));
            return;
        }
    };
    let plan = laft_svc::reconcile::plan(&old, &new);
    let applied = laft_svc::reconcile::apply(
        &plan,
        env.root.path(),
        &env.path("/run/laft/system/services"),
    );
    for name in &plan.reboot {
        env.ui
            .note(format!("services    {name} changes on the next boot"));
    }
    for (name, why) in &applied.failed {
        env.ui.warn(format!("service {name}: {why}"));
    }
    activation.services_failed = applied
        .not_ready
        .into_iter()
        .chain(applied.failed.into_iter().map(|(n, _)| n))
        .collect();
    activation.done.push("services");
}
