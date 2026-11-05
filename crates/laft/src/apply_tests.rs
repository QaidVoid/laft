//! Tests of the settle bookkeeping of an apply run.

use crate::apply::replace_index;
use crate::env::RepoIndex;
use std::collections::BTreeMap;

fn index(name: &str, generation: u64) -> RepoIndex {
    let none: Vec<String> = Vec::new();
    let url = "file:///tmp/none";
    let index = laft_index::Index {
        format: laft_index::Index::FORMAT,
        repo: name.parse().expect("a name"),
        channel: "current".parse().expect("a name"),
        arch: crate::env::ARCH,
        generation,
        timestamp: laft_core::Timestamp::now(),
        expires: laft_core::Timestamp::from_secs(
            laft_core::Timestamp::now().as_secs() + laft_index::MAX_VALIDITY_SECS,
        ),
        defaults: Default::default(),
        packages: Vec::new(),
        yanked: Vec::new(),
        options: BTreeMap::new(),
        recipes: None,
        profile: None,
    };
    RepoIndex {
        name: name.parse().expect("a name"),
        repo: laft_config::Repo {
            url: url.to_owned(),
            mirrors: Vec::new(),
            key: "RWSO9Zvh1ju0E6MO5PDnzQFYnptNxsLpLdwYSfm5Cs4Jc+14umRuka1M"
                .parse()
                .expect("a key"),
            keys: Vec::new(),
            channel: "current".parse().expect("a name"),
            profile: None,
            priority: 1,
            enable: true,
            allow_http: false,
        },
        accepted: laft_index::AcceptedIndex {
            index,
            record: laft_index::IndexRecord {
                channel: "current".parse().expect("a name"),
                profile: None,
                arch: crate::env::ARCH,
                generation,
                blake3: Some(laft_core::Blake3Hash::of(b"x")),
                signer: "0123456789abcdef".parse().expect("a key id"),
            },
            expired: false,
        },
        mirrors: laft_fetch::Mirrors::new(url, &none, true).expect("a mirror"),
        synced: None,
    }
}

#[test]
fn a_second_local_index_replaces_the_first_instead_of_accumulating() {
    let mut indexes = vec![index("core", 1), index("local", 1)];
    replace_index(&mut indexes, index("local", 2));
    let locals: Vec<&RepoIndex> = indexes
        .iter()
        .filter(|i| i.name.as_str() == "local")
        .collect();
    assert_eq!(locals.len(), 1, "one local index: {indexes:?}");
    assert_eq!(locals[0].accepted.index.generation, 2, "the fresh one");
    assert_eq!(indexes[0].name.as_str(), "core", "the others stay");
}
