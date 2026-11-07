//! The option scenario, which `--options` runs.
//!
//! The machine fixes options in its configuration, builds what no
//! repository publishes, and rolls back between the optioned and the
//! default generation. A second machine below `/srv/guest` installs the
//! first machine's local build through `laft repo serve`, following the
//! `loud` profile of the repository, and refuses the served repository
//! when the key does not match. The boots map the acceptance tests of
//! the option spec to real checks.

use std::path::Path;

use crate::qemu::{Accel, Machine};
use crate::{Paths, Result, check, check_absent, fail, login, power_off, shell, start};

/// The key line the second machine's configuration gets instead of the
/// public key of the served repository, which it must refuse.
const WRONG_KEY: &str = "RWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRWRW";

/// The boots of the scenario, on a copy of the installed disk, so that a
/// kept image stays as it was installed.
pub(crate) fn boots(paths: &Paths, accel: Accel, disk: &Path, vars: &Path) -> Result<()> {
    let copy = paths.work.join("options.img");
    let copy_vars = paths.work.join("options-vars.fd");
    let make_copy = |from: &Path, to: &Path| {
        let _ = std::fs::remove_file(to);
        std::fs::copy(from, to).map_err(|e| fail(format!("copying {}: {e}", from.display())))
    };
    let result = make_copy(disk, &copy)
        .and_then(|_| make_copy(vars, &copy_vars))
        .and_then(|_| builds(paths, accel, &copy, &copy_vars))
        .and_then(|_| boots_optioned(paths, accel, &copy, &copy_vars))
        .and_then(|_| boots_default(paths, accel, &copy, &copy_vars))
        .and_then(|_| boots_rolled_back(paths, accel, &copy, &copy_vars));
    for file in [&copy, &copy_vars] {
        let _ = std::fs::remove_file(file);
    }
    result
}

/// Boots the installed system and builds the optioned packages: the
/// refusal without `--build-missing`, the build, the resting second
/// apply, the build view of a dependent, and the second machine.
fn builds(paths: &Paths, accel: Accel, disk: &Path, vars: &Path) -> Result<()> {
    let mut machine = start(paths, accel, disk, vars, "options-1.log", None)?;
    login(&mut machine)?;
    fix_options(&mut machine)?;

    let refused = shell(&mut machine, "laft apply --yes; echo refused-status=$?")?;
    check("the refusal names the missing packages", &refused, "S011")?;
    check_absent("the refusal builds nothing", &refused, "refused-status=0")?;

    let built = shell(
        &mut machine,
        "laft apply --yes --build-missing; s=$?; [ $s -eq 0 ] || \
         find /laft/build/logs -type f -exec cat {} \\;; echo built-status=$s",
    )?;
    check("the optioned apply succeeds", &built, "built-status=0")?;
    check(
        "the optioned apply builds from the snapshot",
        &built,
        "from a recipe snapshot",
    )?;
    let text = shell(&mut machine, "cat /usr/share/greeting/greeting.txt")?;
    check(
        "the optioned greeting is loud, twice",
        &text,
        "The greeting is loud.\r\ntwice\r\n",
    )?;
    let hello = shell(
        &mut machine,
        "cat /usr/share/hello/banner.txt /usr/share/hello/hello.txt",
    )?;
    check(
        "the build view held the optioned libgreet",
        &hello,
        "The banner flies.\r\nshout=true\r\n",
    )?;

    let again = shell(&mut machine, "laft apply --yes; echo again-status=$?")?;
    check("the second apply succeeds", &again, "again-status=0")?;
    check_absent("the second apply builds nothing", &again, "recipe snapshot")?;

    serve_and_second_machine(&mut machine)?;
    power_off(&mut machine)
}

/// Serves the local repository and installs its packages on a second
/// machine, with the profile of the repository, and refuses the served
/// repository with a wrong key.
fn serve_and_second_machine(machine: &mut Machine) -> Result<()> {
    // The minimal system installs no network package set, so nothing
    // brings the loopback interface up, and the served repository listens
    // on it.
    let up = shell(machine, "ip link set lo up; echo lo-status=$?")?;
    check("the loopback interface is up", &up, "lo-status=0")?;
    let serve = shell(
        machine,
        "laft repo serve --listen 127.0.0.1:7979 >/tmp/serve.log 2>&1 & echo serve-pid=$!",
    )?;
    let pid = serve
        .lines()
        .find_map(|line| line.strip_prefix("serve-pid="))
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .ok_or_else(|| fail(format!("the repository server did not start: {serve}")))?;
    let config = shell(machine, &guest_config())?;
    check("the second machine is configured", &config, "config-ok\r\n")?;

    let guest = shell(
        machine,
        "laft --root /srv/guest apply --yes; echo guest-status=$?",
    )?;
    check(
        "the second machine installs from the served repository",
        &guest,
        "guest-status=0",
    )?;
    check_absent("the second machine builds nothing", &guest, "building")?;
    let text = shell(
        machine,
        "cat /srv/guest/laft/gens/*/usr/share/greeting/greeting.txt",
    )?;
    check(
        "the second machine took the profile's option set",
        &text,
        "The greeting is loud.",
    )?;

    let wrong = shell(
        machine,
        &format!(
            "sed -i 's/^    key = \"[^\"]*\"$/    key = \"{WRONG_KEY}\"/' \
             /srv/guest/etc/laft/system.stav && \
             laft --root /srv/guest repo reset local && \
             laft --root /srv/guest repo sync local; echo wrong-status=$?"
        ),
    )?;
    check_absent(
        "the served repository refuses the wrong key",
        &wrong,
        "wrong-status=0",
    )?;

    let stopped = shell(machine, &format!("kill {pid}; echo killed-status=$?"))?;
    check("the server stopped", &stopped, "killed-status=0")?;

    Ok(())
}

/// The shell command that writes the configuration of the second
/// machine: the profile of the core repository and the served local
/// repository of the first machine.
fn guest_config() -> String {
    String::new()
        + "corekey=$(sed -n 's/^    key = \"\\(.*\\)\"$/\\1/p' /etc/laft/system.stav | head -1) && "
        + "localkey=$(laft repo key local) && "
        + "mkdir -p /srv/guest/etc/laft && "
        + "printf '%s\\n' 'system {' '  hostname = \"guest\"' \
           '  packages = [busybox:static, greeting]' \
           '  repo core {' '    url = \"file:///repo\"' \"    key = \\\"$corekey\\\"\" \
           '    profile = \"loud\"' '  }' '  repo local {' \
           '    url = \"http://127.0.0.1:7979\"' '    allow_http = true' \
           \"    key = \\\"$localkey\\\"\" '  }' \
           '  boot { microcode = force(off) }' \
           '  fs \"/\" { device = \"UUID=11111111-1111-1111-1111-111111111111\", \
           fstype = ext4, fsck = false }' \
           '  fs \"/boot\" { device = \"UUID=1A2B-3C4D\", fstype = vfat }' '}' \
           > /srv/guest/etc/laft/system.stav && echo config-ok"
}

/// Boots the optioned generation, which the previous boot made the
/// default, and applies the default option set.
fn boots_optioned(paths: &Paths, accel: Accel, disk: &Path, vars: &Path) -> Result<()> {
    let mut machine = start(paths, accel, disk, vars, "options-2.log", None)?;
    machine.expect(
        "the optioned generation is the default",
        "laft-boot: switching to generation",
    )?;
    login(&mut machine)?;
    let text = shell(&mut machine, "cat /usr/share/greeting/greeting.txt")?;
    check(
        "the optioned generation boots loud",
        &text,
        "The greeting is loud.",
    )?;
    let hello = shell(
        &mut machine,
        "cat /usr/share/hello/banner.txt /usr/share/hello/hello.txt",
    )?;
    check(
        "the optioned generation boots hello",
        &hello,
        "The banner flies.\r\nshout=true\r\n",
    )?;

    let calm = shell(
        &mut machine,
        "sed -i -e 's/mood = loud/mood = calm/' -e 's/twice = true/twice = false/' \
         /etc/laft/system.stav && laft apply --yes --build-missing; echo calm-status=$?",
    )?;
    check("the default apply succeeds", &calm, "calm-status=0")?;
    check_absent(
        "the default apply installs the published packages",
        &calm,
        "recipe snapshot",
    )?;
    let text = shell(&mut machine, "cat /usr/share/greeting/greeting.txt")?;
    check(
        "the default greeting is calm",
        &text,
        "The greeting is calm.\r\n",
    )?;
    power_off(&mut machine)
}

/// Boots the default generation and rolls back to the optioned one.
fn boots_default(paths: &Paths, accel: Accel, disk: &Path, vars: &Path) -> Result<()> {
    let mut machine = start(paths, accel, disk, vars, "options-3.log", None)?;
    machine.expect(
        "the default generation is the default",
        "laft-boot: switching to generation",
    )?;
    login(&mut machine)?;
    let text = shell(&mut machine, "cat /usr/share/greeting/greeting.txt")?;
    check(
        "the default generation boots calm",
        &text,
        "The greeting is calm.",
    )?;

    let before = shell(&mut machine, "readlink /run/laft/system")?;
    let rollback = shell(&mut machine, "laft rollback; echo rollback-status=$?")?;
    check("the rollback succeeds", &rollback, "rollback-status=0")?;
    let after = shell(&mut machine, "readlink /run/laft/system")?;
    check_absent(
        "the rollback leaves the optioned generation active",
        &after,
        before.trim(),
    )?;
    let text = shell(&mut machine, "cat /usr/share/greeting/greeting.txt")?;
    check(
        "the rolled back generation is loud",
        &text,
        "The greeting is loud.",
    )?;
    power_off(&mut machine)
}

/// Boots the generation the rollback made the default.
fn boots_rolled_back(paths: &Paths, accel: Accel, disk: &Path, vars: &Path) -> Result<()> {
    let mut machine = start(paths, accel, disk, vars, "options-4.log", None)?;
    login(&mut machine)?;
    let text = shell(&mut machine, "cat /usr/share/greeting/greeting.txt")?;
    check(
        "the rolled back generation boots loud",
        &text,
        "The greeting is loud.",
    )?;
    let hello = shell(
        &mut machine,
        "cat /usr/share/hello/banner.txt /usr/share/hello/hello.txt",
    )?;
    check(
        "the rolled back generation boots hello",
        &hello,
        "The banner flies.\r\nshout=true\r\n",
    )?;
    power_off(&mut machine)
}

/// Adds the packages of the scenario and fixes their options in the
/// configuration of the machine.
fn fix_options(machine: &mut Machine) -> Result<()> {
    shell(
        machine,
        "sed -i -e 's/laft\\]/laft, greeting, hello]/' -e '$d' /etc/laft/system.stav && \
         printf '%s\\n' '  package {' '    greeting {' '      mood = loud' \
         '      twice = true' '    }' '    libgreet {' '      shout = true' '    }' \
         '    hello {' '      banner = true' '    }' '  }' '}' \
         >> /etc/laft/system.stav && echo options-ok",
    )?;
    Ok(())
}
