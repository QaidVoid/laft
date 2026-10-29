use super::*;
use crate::test_support::{elf_fixture, mode, recipe, write_tree};

fn tools_missing() -> bool {
    let missing = ["objcopy", "strip", "bash"]
        .iter()
        .any(|t| !Path::new("/usr/bin").join(t).exists());
    if missing {
        println!("skipping: objcopy, strip or bash is missing from /usr/bin");
    }
    missing
}

fn setup(debug_split: bool) -> (laft_test::TempDir, ElfFiles, Executor, BuildOptions) {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    write_tree(&out, &[("usr/bin/program", &elf_fixture("program"))]);
    fsx::set_mode(&out.join("usr/bin/program"), 0o555).unwrap();
    let elves = crate::post::scan_elf(&out, &mut Vec::new()).unwrap();
    let exec = Executor::direct("/", dir.path(), &out, dir.path().join("logs"));
    let mut options = recipe("x").options;
    options.debug_split = debug_split;
    (dir, elves, exec, options)
}

#[test]
fn debug_info_is_split_off_and_linked() {
    if tools_missing() {
        return;
    }
    let (dir, elves, exec, options) = setup(true);
    let out = dir.path().join("out");
    strip(&out, &elves, &options, &exec).expect("stripping works");
    let id = elves["usr/bin/program"]
        .build_id
        .clone()
        .expect("a build id");
    let debug = out.join(debug_path(&id).unwrap());
    assert!(debug.is_file(), "{} exists", debug.display());
    assert_eq!(mode(&debug), 0o444);
    assert_eq!(mode(&out.join("usr/bin/program")), 0o555);
    let program = fs::read(out.join("usr/bin/program")).unwrap();
    assert!(program.windows(14).any(|w| w == b".gnu_debuglink"));
}

#[test]
fn without_debug_split_nothing_goes_to_usr_lib_debug() {
    if tools_missing() {
        return;
    }
    let (dir, elves, exec, options) = setup(false);
    let out = dir.path().join("out");
    strip(&out, &elves, &options, &exec).unwrap();
    assert!(!out.join(DEBUG_DIR).exists());
}

#[test]
fn nothing_changes_when_stripping_is_off() {
    let (dir, elves, exec, mut options) = setup(true);
    options.strip = false;
    let out = dir.path().join("out");
    strip(&out, &elves, &options, &exec).unwrap();
    assert_eq!(
        fs::read(out.join("usr/bin/program")).unwrap(),
        elf_fixture("program")
    );
    assert!(!dir.path().join("logs").exists());
}

#[test]
fn debug_files_live_below_their_build_id() {
    assert_eq!(
        debug_path(&[0xab, 0xcd, 0xef]).as_deref(),
        Some("usr/lib/debug/.build-id/ab/cdef.debug")
    );
    assert_eq!(debug_path(&[0xab]), None);
}

#[test]
fn a_file_linked_under_two_names_is_stripped_once() {
    if tools_missing() {
        return;
    }
    let (dir, _, exec, options) = setup(true);
    let out = dir.path().join("out");
    fs::hard_link(out.join("usr/bin/program"), out.join("usr/bin/alias")).unwrap();
    let elves = crate::post::scan_elf(&out, &mut Vec::new()).unwrap();
    strip(&out, &elves, &options, &exec).expect("each file is stripped once");
    let program = fs::read(out.join("usr/bin/program")).unwrap();
    let debuglinks = program
        .windows(14)
        .filter(|w| *w == b".gnu_debuglink")
        .count();
    assert_eq!(debuglinks, 1);
}

#[test]
fn a_failed_tool_leaves_the_file_mode_alone() {
    let (dir, elves, _, options) = setup(false);
    let out = dir.path().join("out");
    let empty = dir.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    let exec = Executor::direct(&empty, dir.path(), &out, dir.path().join("logs"));
    strip(&out, &elves, &options, &exec).expect_err("no tools exist");
    assert_eq!(mode(&out.join("usr/bin/program")), 0o555);
}
