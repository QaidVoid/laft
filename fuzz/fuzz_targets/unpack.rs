//! Feeds arbitrary bytes to source extraction as a tar or zip archive and
//! checks that nothing lands outside the source tree.
//!
//! The first input byte picks the format and whether one leading component
//! is stripped. The source tree sits one level below a directory that
//! must hold nothing else afterwards, so an entry that escapes is caught
//! there instead of reaching the rest of the system.

#![no_main]

use std::fs;

use laft_build::{Fetch, Source, unpack};
use laft_core::Sha256Hash;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, archive)) = data.split_first() else {
        return;
    };
    let file = if choice & 1 == 0 { "x.tar" } else { "x.zip" };
    let source = Source {
        label: "main".to_owned(),
        fetch: Fetch::Url {
            urls: vec![format!("https://example.org/{file}")],
            sha256: Sha256Hash::from_bytes([0; 32]),
            file: file.to_owned(),
            extract: true,
        },
        dest: String::new(),
        strip_components: (choice & 2 != 0).then_some(1),
    };
    let dir = tempfile::tempdir().expect("a temporary directory can be created");
    let path = dir.path().join(file);
    fs::write(&path, archive).expect("the archive can be written");
    let outer = dir.path().join("outer");
    let (src, scratch) = (outer.join("src"), outer.join("scratch"));
    fs::create_dir_all(&src).expect("the source tree can be created");
    fs::create_dir_all(&scratch).expect("the scratch directory can be created");
    let _ = unpack(&source, &path, &src, &scratch);
    let mut names: Vec<_> = fs::read_dir(&outer)
        .expect("the outer directory is readable")
        .map(|e| e.expect("the outer directory is readable").file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["scratch", "src"]);
    assert_eq!(
        fs::read_dir(&scratch)
            .expect("the scratch directory is readable")
            .count(),
        0
    );
});
