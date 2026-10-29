use super::*;

#[test]
fn hardware_stav_declares_filesystems_swap_luks_and_boot() {
    let probe = Probe {
        filesystems: vec![
            ProbedFs {
                mountpoint: "/".into(),
                device: "/dev/mapper/cryptroot".into(),
                fstype: "ext4".into(),
            },
            ProbedFs {
                mountpoint: "/boot".into(),
                device: "UUID=1A2B-3C4D".into(),
                fstype: "vfat".into(),
            },
            ProbedFs {
                mountpoint: "/data".into(),
                device: "UUID=9".into(),
                fstype: "zfs".into(),
            },
        ],
        swap: vec!["UUID=5".into()],
        luks: BTreeMap::from([("cryptroot".to_owned(), "UUID=7".to_owned())]),
        microcode: Some("amd"),
        modules: BTreeSet::from(["nvme".to_owned()]),
    };
    let text = hardware_stav(&probe);
    assert!(
        text.contains("fs \"/\" {\n  device = \"/dev/mapper/cryptroot\"\n  fstype = ext4\n}"),
        "{text}"
    );
    assert!(text.contains("fstype = other(name = \"zfs\")"), "{text}");
    assert!(text.contains("swap \"UUID=5\" {}"), "{text}");
    assert!(
        text.contains("luks \"cryptroot\" { device = \"UUID=7\" }"),
        "{text}"
    );
    assert!(text.contains("microcode = amd"), "{text}");
    assert!(text.contains("initrd { modules = [\"nvme\"] }"), "{text}");
}

#[test]
fn the_generated_configuration_loads_with_its_hardware() {
    let dir = laft_test::tempdir();
    fs::write(dir.path().join("system.stav"), TEMPLATE).expect("writable");
    let probe = Probe {
        filesystems: vec![
            ProbedFs {
                mountpoint: "/".into(),
                device: "UUID=1".into(),
                fstype: "ext4".into(),
            },
            ProbedFs {
                mountpoint: "/boot".into(),
                device: "UUID=2".into(),
                fstype: "vfat".into(),
            },
        ],
        ..Probe::default()
    };
    fs::write(dir.path().join("hardware.stav"), hardware_stav(&probe)).expect("writable");
    let loaded = laft_config::load(dir.path(), &|_| Ok(())).expect("the template is valid");
    assert_eq!(loaded.config.etc.hostname, "laft");
    assert_eq!(loaded.config.boot.fs["/"].fstype, "ext4");
}

#[test]
fn a_target_may_hold_only_the_esp_lost_and_found_and_the_configuration() {
    let dir = laft_test::tempdir();
    for path in ["boot/EFI", "lost+found", "etc/laft"] {
        fs::create_dir_all(dir.path().join(path)).expect("writable");
    }
    check_empty(dir.path()).expect("an empty target");
    fs::create_dir_all(dir.path().join("etc/ssh")).expect("writable");
    let failure = check_empty(dir.path()).expect_err("etc/ssh is in the way");
    assert!(failure.context.iter().any(|c| c.contains("etc/ssh")));
}

#[test]
fn the_layout_links_usr_to_the_active_generation() {
    let dir = laft_test::tempdir();
    create_layout(dir.path()).expect("created");
    assert_eq!(
        fs::read_link(dir.path().join("usr")).expect("a link"),
        Path::new("/run/laft/system/usr")
    );
    assert_eq!(
        fs::read_link(dir.path().join("etc/static")).expect("a link"),
        Path::new("/run/laft/system/etc")
    );
    create_layout(dir.path()).expect("creating it again changes nothing");
}

#[test]
fn probed_names_cannot_break_out_of_their_strings() {
    let probe = Probe {
        filesystems: vec![ProbedFs {
            mountpoint: "/mnt/a\"b#{x}\\c".into(),
            device: "LABEL=x\" }\nboot { }".into(),
            fstype: "ext4".into(),
        }],
        ..Probe::default()
    };
    let text = hardware_stav(&probe);
    assert!(text.contains("fs \"/mnt/a\\\"b\\#{x}\\\\c\" {"), "{text}");
    assert!(
        text.contains("device = \"LABEL=x\\\" }\\nboot { }\""),
        "{text}"
    );
    let formatted = stav::format(&format!("system {{\n{text}\n}}\n"));
    assert!(formatted.is_ok(), "{text}");
}
