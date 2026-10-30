//! Boot entries, services, activation at boot, the installer and the
//! other commands that manage a system, below `--root`.

mod support;

use std::fs;
use std::os::unix::fs::symlink;

use support::{pkg, run};

fn hello() -> support::Pkg {
    pkg("hello", "1.0").file("usr/bin/hello", b"#!/bin/sh\necho hello\n")
}

#[test]
fn boot_entries_trials_and_activation_at_boot() {
    let fx = fixture!(hello());
    fx.configure(", hello", "");
    fx.laft(&["apply", "-y"]).expect(0);
    fx.configure("", "");
    fx.laft(&["apply", "-y"]).expect(0);

    let list = fx.laft(&["boot", "list"]).expect(0);
    let rows: Vec<&str> = list.stdout.lines().collect();
    assert!(
        rows[1].starts_with("1  2    6.1.0-laft1  yes"),
        "{}",
        list.stdout
    );
    assert!(
        rows[2].starts_with("2  1    6.1.0-laft1"),
        "{}",
        list.stdout
    );
    let show = fx.laft(&["boot", "show", "2"]).expect(0);
    assert!(show.stdout.contains("status      trial"), "{}", show.stdout);
    assert!(
        show.stdout
            .contains("cmdline     laft.gen=2 quiet loglevel=3"),
        "{}",
        show.stdout
    );
    let log = fx.laft(&["boot", "log"]).expect(0);
    assert!(
        log.stdout
            .contains("trial       generation 2, 3 tries left"),
        "{}",
        log.stdout
    );

    fx.laft(&["boot", "set-default", "1", "-y"]).expect(0);
    assert_eq!(
        fs::read_link(fx.root().join("laft/gens/default"))
            .expect("a link")
            .to_str(),
        Some("1")
    );
    let json = fx.laft(&["boot", "list", "--json"]).expect(0).json();
    assert_eq!(json["entries"][1]["default"], true);

    fs::create_dir_all(fx.root().join("run/laft/mounts")).expect("writable");
    symlink("mounts/1", fx.root().join("run/laft/booted")).expect("can link");
    fx.laft(&["boot", "mark-good"]).expect(0);
    let show = fx.laft(&["boot", "show", "1"]).expect(0);
    assert!(show.stdout.contains("status      good"), "{}", show.stdout);

    fs::remove_file(fx.root().join("etc/hostname")).expect("managed");
    fx.laft(&["activate", "--boot"]).expect(0);
    assert_eq!(
        fs::read_link(fx.root().join("etc/hostname"))
            .expect("relinked")
            .to_str(),
        Some("/etc/static/hostname")
    );
    fx.laft(&["boot", "install"]).expect(0);
}

#[test]
fn services_are_described_from_the_active_generation() {
    let fx = fixture!(hello());
    fx.configure(", hello", "");
    fx.laft(&["apply", "-y"]).expect(0);

    let status = fx.laft(&["svc", "status"]).expect(0);
    assert!(status.stdout.starts_with("SERVICE"), "{}", status.stdout);
    assert!(status.stdout.contains("turnstiled"), "{}", status.stdout);
    let json = fx
        .laft(&["svc", "status", "--json", "turnstiled"])
        .expect(0)
        .json();
    assert_eq!(json["services"][0]["state"], "down");
    let show = fx.laft(&["svc", "show", "turnstiled"]).expect(0);
    assert!(
        show.stdout.contains("exec        /usr/bin/turnstiled"),
        "{}",
        show.stdout
    );
    assert!(
        show.stdout.contains("restart     always\n"),
        "{}",
        show.stdout
    );
    fx.laft(&["svc", "show", "nothing"]).expect(2);
    fx.laft(&["svc", "start", "turnstiled"]).expect(1);

    let log = fx.root().join("var/log/turnstiled/current");
    fs::create_dir_all(log.parent().expect("a parent")).expect("writable");
    let label = format!(
        "@{:016x}00000000",
        4_611_686_018_427_387_914u64 + 1_761_652_800
    );
    fs::write(&log, format!("{label} first\n{label} second\n")).expect("writable");
    let printed = fx.laft(&["svc", "log", "turnstiled", "-n", "1"]).expect(0);
    assert!(printed.stdout.ends_with(" second\n"), "{}", printed.stdout);
    assert!(
        printed.stdout.starts_with("2025-10-2"),
        "{}",
        printed.stdout
    );
    let console = fx.laft(&["svc", "log", "hostname"]).expect(1);
    assert!(
        console
            .stderr
            .contains("service hostname logs to the console, so it has no log file"),
        "{}",
        console.stderr
    );

    let launcher = fx.laft(&["svc", "exec", "--backend", "runit", "nothing"]);
    assert_eq!(launcher.code, 111, "{}", launcher.stderr);
}

#[test]
fn the_installer_generates_a_configuration_and_checks_its_target() {
    let dir = laft_test::tempdir();
    let target = dir.path().join("target");
    fs::create_dir_all(target.join("boot")).expect("writable");
    let root = target.to_str().expect("UTF-8");
    run(&["--root", root, "install", "--generate-config"]).expect(0);
    let config = fs::read_to_string(target.join("etc/laft/system.stav")).expect("written");
    assert!(config.contains("include \"./hardware.stav\""));
    assert!(target.join("etc/laft/hardware.stav").exists());
    run(&["--root", root, "install", "--generate-config"]).expect(1);
    run(&["--root", root, "hardware", "--write"]).expect(1);
    fs::remove_file(target.join("etc/laft/hardware.stav")).expect("removable");
    let written = run(&["--root", root, "--json", "hardware", "--write"]).expect(0);
    assert!(
        written.stdout.contains("hardware.stav"),
        "{}",
        written.stdout
    );
    assert!(target.join("etc/laft/hardware.stav").exists());

    let refused = run(&["--root", root, "install", "-y"]).expect(1);
    assert!(refused.stderr.contains("FAT"), "{}", refused.stderr);
    fs::create_dir_all(target.join("home")).expect("writable");
    let busy = run(&["--root", root, "install", "-y"]).expect(1);
    assert!(busy.stderr.contains("is not empty"), "{}", busy.stderr);
    run(&["install"]).expect(2);
    run(&["--root", root, "enter", "dev", "--", "true"]).expect(2);
}

#[test]
fn hardware_completions_and_rescue_need_nothing_but_the_command_line() {
    let hardware = run(&["hardware"]).expect(0);
    assert!(hardware.stdout.contains("boot {"), "{}", hardware.stdout);
    let probe = run(&["hardware", "--json"]).expect(0).json();
    assert_eq!(probe["laft_json"], 1);
    let completions = run(&["completions", "bash"]).expect(0);
    assert!(
        completions.stdout.contains("laft"),
        "completions are printed"
    );
    if !rustix::process::geteuid().is_root() {
        let dir = laft_test::tempdir();
        run(&["--root", dir.path().to_str().expect("UTF-8"), "rescue"]).expect(1);
    }
    run(&["internal", "worker"]).expect(2);
}

#[test]
fn every_command_uses_the_configured_efi_system_partition() {
    let fx = fixture!(hello());
    fs::create_dir_all(fx.root().join("efi")).expect("writable");
    let body =
        "  boot { esp = \"/efi\" }\n  fs \"/efi\" { device = \"UUID=AB-CD\", fstype = vfat }";
    fx.configure(", hello", body);
    fx.laft(&["apply", "-y"]).expect(0);
    assert!(fx.read("efi/limine.conf").contains("Laft 1"));
    fx.configure("", body);
    fx.laft(&["apply", "-y"]).expect(0);

    fs::remove_file(fx.root().join("efi/limine.conf")).expect("removable");
    fx.laft(&["boot", "install"]).expect(0);
    assert!(fx.read("efi/limine.conf").contains("Laft 2"));
    let list = fx.laft(&["boot", "list"]).expect(0);
    assert!(list.stdout.lines().count() > 2, "{}", list.stdout);
    fx.laft(&["rollback", "-y"]).expect(0);
    fx.laft(&["gens", "rm", "2", "-y"]).expect(0);
    assert!(!fx.read("efi/limine.conf").contains("Laft 2"));
    assert!(
        !fx.root().join("boot/limine.conf").exists(),
        "nothing is written to /boot"
    );
}

#[test]
fn gc_refuses_to_run_with_a_broken_configuration() {
    let fx = fixture!(hello());
    fx.configure(", hello", "");
    fx.laft(&["apply", "-y"]).expect(0);
    fx.configure("", "");
    fx.laft(&["apply", "-y"]).expect(0);
    fs::write(
        fx.root().join("etc/laft/system.stav"),
        "system { nonsense }\n",
    )
    .expect("writable");
    let gc = fx.laft(&["gc", "--dry-run"]);
    assert_ne!(gc.code, 0, "{}", gc.stdout);
    let rm = fx.laft(&["gens", "rm", "1", "-y"]);
    assert_ne!(rm.code, 0, "{}", rm.stdout);
    assert!(
        fx.root().join("laft/gens/1").exists(),
        "nothing was deleted"
    );
}
