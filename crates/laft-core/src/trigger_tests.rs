use super::*;

#[test]
fn double_star_matches_any_number_of_components() {
    let pattern = "usr/lib/modules/*/**/*.ko";
    assert!(glob_matches(pattern, "usr/lib/modules/6.1/a.ko"));
    assert!(glob_matches(
        pattern,
        "usr/lib/modules/6.1/kernel/fs/ext4/ext4.ko"
    ));
    assert!(!glob_matches(pattern, "usr/lib/modules/a.ko"));
    assert!(!glob_matches(
        pattern,
        "usr/lib/modules/6.1/kernel/a.ko.zst"
    ));
}

#[test]
fn single_star_stays_within_one_component() {
    assert!(glob_matches(
        "usr/share/info/*.info*",
        "usr/share/info/gcc.info.gz"
    ));
    assert!(glob_matches(
        "usr/share/info/*.info*",
        "usr/share/info/gcc.info"
    ));
    assert!(!glob_matches(
        "usr/share/info/*.info*",
        "usr/share/info/sub/gcc.info"
    ));
    assert!(!glob_matches(
        "usr/share/info/*.info*",
        "usr/share/info/dir"
    ));
    assert!(glob_matches("a*b*c", "abc"));
    assert!(glob_matches("a*b*c", "axxbyybc"));
    assert!(!glob_matches("a*b*c", "axxbyyb"));
}

#[test]
fn triggers_match_their_payload_paths() {
    let fonts = trigger("fontconfig").unwrap();
    assert!(fonts.activated_by("usr/share/fonts/TTF/DejaVuSans.ttf"));
    assert!(!fonts.activated_by("usr/share/fontsx/a.ttf"));
    let icons = trigger("icons").unwrap();
    assert!(icons.activated_by("usr/share/icons/hicolor/index.theme"));
    assert!(!icons.activated_by("usr/share/iconsx/hicolor/index.theme"));
}

#[test]
fn table_names_are_unique_and_valid_package_names() {
    let mut names: Vec<&str> = TRIGGERS.iter().map(|t| t.name).collect();
    for name in &names {
        assert!(name.parse::<crate::Name>().is_ok(), "{name}");
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), TRIGGERS.len());
}

#[test]
fn only_depmod_is_required() {
    let required: Vec<&str> = TRIGGERS
        .iter()
        .filter(|t| t.required)
        .map(|t| t.name)
        .collect();
    assert_eq!(required, ["depmod"]);
}

#[test]
fn every_output_lies_in_the_view_tree() {
    for trigger in TRIGGERS {
        assert!(!trigger.tools.is_empty(), "{}", trigger.name);
        for output in trigger.outputs {
            assert!(output.starts_with("usr/"), "{output}");
            assert!(!output.split('/').any(|c| c == ".." || c.is_empty()));
        }
    }
}

#[test]
fn generated_files_are_recognized_and_inputs_are_not() {
    let generated = |path: &str| TRIGGERS.iter().any(|t| t.generates(path));
    for path in [
        "usr/lib/modules/6.17.5-laft1/modules.dep",
        "usr/lib/modules/6.17.5-laft1/modules.alias.bin",
        "usr/lib/fontconfig/cache/abc-le64.cache-9",
        "usr/share/glib-2.0/schemas/gschemas.compiled",
        "usr/share/mime/mime.cache",
        "usr/share/mime/text/plain.xml",
        "usr/share/icons/hicolor/icon-theme.cache",
        "usr/lib/udev/hwdb.bin",
        "usr/share/info/dir",
    ] {
        assert!(generated(path), "{path}");
    }
    for path in [
        "usr/lib/modules/6.17.5-laft1/kernel/fs/ext4/ext4.ko.zst",
        "usr/lib/modules/6.17.5-laft1/modules.order",
        "usr/share/glib-2.0/schemas/org.gtk.Settings.xml",
        "usr/share/mime/packages/freedesktop.org.xml",
        "usr/share/icons/hicolor/index.theme",
        "usr/share/info/gcc.info",
    ] {
        assert!(!generated(path), "{path}");
    }
}

#[test]
fn generated_files_lie_in_the_output_directories() {
    for trigger in TRIGGERS {
        for pattern in trigger.generated {
            let pattern = pattern.strip_prefix('!').unwrap_or(pattern);
            let within = trigger.outputs.iter().any(|output| {
                let output = output.replace("{}", "*");
                pattern
                    .strip_prefix(output.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            });
            assert!(within, "{}: {pattern}", trigger.name);
        }
    }
}
