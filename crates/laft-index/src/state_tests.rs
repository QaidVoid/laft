use std::fs;

use laft_core::Timestamp;
use laft_test::TempDir;

use super::*;
use crate::test_support::{DAY, Signed, T0, TestKey, index, keys, now, repo};

/// A client with an empty state directory, a root key and an online key
/// listed in version 1 of `keys.json`.
struct Client {
    _dir: TempDir,
    state: IndexState,
    root: TestKey,
    online: TestKey,
    repo: Repo,
}

impl Client {
    fn new() -> Self {
        let dir = laft_test::tempdir();
        let root = TestKey::generate();
        Self {
            state: IndexState::new(dir.path().join("state/index")),
            _dir: dir,
            online: TestKey::generate(),
            repo: repo(&root),
            root,
        }
    }

    fn keys_v1(&self) -> Keys {
        keys(1, vec![self.online.online()], vec![])
    }

    fn accept_keys(&self, keys: &Keys) -> Result<Trust, Error> {
        let signed = self.root.sign_keys(keys);
        self.state.accept_keys(
            &self.repo,
            KeysFetch::Found {
                json: &signed.bytes,
                signature: &signed.signature,
            },
        )
    }

    fn direct(&self) -> Trust {
        self.state
            .accept_keys(&self.repo, KeysFetch::NotFound)
            .expect("a repository without keys.json is in direct mode")
    }

    fn accept(&self, trust: &Trust, signed: &Signed) -> Result<AcceptedIndex, Error> {
        self.state.accept_index(
            &self.repo,
            trust,
            &signed.bytes,
            &signed.signature,
            now(),
            false,
        )
    }

    fn generation(&self) -> Option<u64> {
        self.state
            .index_records(&self.repo.name)
            .expect("readable records")
            .first()
            .map(|r| r.generation)
    }
}

#[track_caller]
fn assert_kind(
    result: Result<impl std::fmt::Debug, Error>,
    matches: impl Fn(&ErrorKind) -> bool,
) -> Error {
    let err = result.expect_err("the document must be rejected");
    assert!(matches(err.kind()), "unexpected error: {err}");
    assert_eq!(err.repo().map(|r| r.as_str()), Some("core"), "{err}");
    err
}

#[test]
fn index_signed_by_a_listed_online_key_is_accepted_and_recorded() {
    let client = Client::new();
    let trust = client
        .accept_keys(&client.keys_v1())
        .expect("root-signed keys.json");
    let accepted = client
        .accept(&trust, &client.online.sign_index(&index(T0)))
        .expect("signed by a listed key");
    assert_eq!(accepted.index, index(T0));
    assert!(!accepted.expired);
    assert_eq!(accepted.record.signer, client.online.id());
    assert_eq!(
        client
            .state
            .index_records(&client.repo.name)
            .expect("records"),
        [accepted.record]
    );
    let keys_record = client.state.keys_record(&client.repo.name).expect("record");
    assert_eq!(
        keys_record.map(|r| (r.version, r.root)),
        Some((1, client.root.id()))
    );
}

#[test]
fn tampered_index_is_rejected() {
    let client = Client::new();
    let trust = client.direct();
    let mut signed = client.root.sign_index(&index(T0));
    let last = signed.bytes.len() - 1;
    signed.bytes[last] ^= 1;
    assert_kind(client.accept(&trust, &signed), |k| {
        matches!(
            k,
            ErrorKind::BadSignature {
                document: "index",
                ..
            }
        )
    });
    assert_eq!(client.generation(), None);
}

#[test]
fn tampered_signature_is_rejected() {
    let client = Client::new();
    let trust = client.direct();
    let signed = client.root.sign_index(&index(T0));
    let text = String::from_utf8(signed.signature.clone()).expect("text");

    let raised = Signed {
        bytes: signed.bytes.clone(),
        signature: text
            .replace(
                &format!("generation={T0}"),
                &format!("generation={}", T0 + 1),
            )
            .into_bytes(),
    };
    assert_kind(client.accept(&trust, &raised), |k| {
        matches!(k, ErrorKind::BadSignature { .. })
    });

    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    // Character 20 of the base64 line encodes Ed25519 signature bytes.
    let flipped = if lines[1].as_bytes()[20] == b'A' {
        "B"
    } else {
        "A"
    };
    lines[1].replace_range(20..21, flipped);
    let garbled = Signed {
        bytes: signed.bytes,
        signature: lines.join("\n").into_bytes(),
    };
    assert_kind(client.accept(&trust, &garbled), |k| {
        matches!(k, ErrorKind::BadSignature { .. })
    });
    assert_eq!(client.generation(), None);
}

#[test]
fn malformed_signature_file_is_rejected() {
    let client = Client::new();
    let trust = client.direct();
    let signed = client.root.sign_index(&index(T0));
    let garbage = Signed {
        bytes: signed.bytes,
        signature: b"untrusted comment: x\nnot base64\n".to_vec(),
    };
    assert_kind(client.accept(&trust, &garbage), |k| {
        matches!(
            k,
            ErrorKind::Invalid {
                what: "signature",
                ..
            }
        )
    });
}

#[test]
fn lower_index_generation_is_rejected() {
    let client = Client::new();
    let trust = client.direct();
    client
        .accept(&trust, &client.root.sign_index(&index(T0 + 100)))
        .expect("first index");
    let err = assert_kind(
        client.accept(&trust, &client.root.sign_index(&index(T0))),
        |k| matches!(k, ErrorKind::Rollback { generation: T0, accepted, .. } if *accepted == T0 + 100),
    );
    assert!(err.to_string().contains("rollback"), "{err}");
    assert_eq!(client.generation(), Some(T0 + 100));
}

#[test]
fn equal_generation_is_accepted_only_with_identical_bytes() {
    let client = Client::new();
    let trust = client.direct();
    let signed = client.root.sign_index(&index(T0));
    client.accept(&trust, &signed).expect("first index");
    client
        .accept(&trust, &signed)
        .expect("the same index again");

    let mut other = index(T0);
    other.packages.pop();
    assert_kind(
        client.accept(&trust, &client.root.sign_index(&other)),
        |k| matches!(k, ErrorKind::Replaced { generation: T0, .. }),
    );
}

#[test]
fn expired_index_is_rejected_unless_allowed() {
    let client = Client::new();
    let trust = client.direct();
    let signed = client.root.sign_index(&index(T0));
    let late = Timestamp::from_secs(T0 + 15 * DAY);
    let accept = |allow| {
        client.state.accept_index(
            &client.repo,
            &trust,
            &signed.bytes,
            &signed.signature,
            late,
            allow,
        )
    };
    let err = assert_kind(accept(false), |k| {
        matches!(k, ErrorKind::Expired { generation: T0, .. })
    });
    assert!(err.to_string().contains("freeze"), "{err}");
    assert_eq!(client.generation(), None);

    let accepted = accept(true).expect("expired indexes may be allowed");
    assert!(accepted.expired);
    assert_eq!(client.generation(), Some(T0));
}

#[test]
fn index_for_another_channel_is_rejected() {
    let client = Client::new();
    let trust = client.direct();
    let mut testing = index(T0);
    testing.channel = "testing".parse().expect("valid name");
    let err = assert_kind(
        client.accept(&trust, &client.root.sign_index(&testing)),
        |k| {
            matches!(k, ErrorKind::WrongTarget { field: "channel", expected, actual, .. }
            if expected == "current" && actual == "testing")
        },
    );
    assert!(err.to_string().contains("channel testing"), "{err}");
}

#[test]
fn index_for_another_repository_is_rejected() {
    let client = Client::new();
    let trust = client.direct();
    let mut other = index(T0);
    other.repo = "mine".parse().expect("valid name");
    assert_kind(
        client.accept(&trust, &client.root.sign_index(&other)),
        |k| matches!(k, ErrorKind::WrongTarget { field: "repo", .. }),
    );
}

#[test]
fn content_that_differs_from_the_trusted_comment_is_rejected() {
    let client = Client::new();
    let trust = client.direct();
    let content = index(T0);
    let bytes = content.encode().expect("valid index");
    let mut comment = content.comment();
    comment.generation = T0 + 50;
    let signed = Signed {
        signature: client.root.sign(&bytes, &comment.to_string()),
        bytes,
    };
    assert_kind(client.accept(&trust, &signed), |k| {
        matches!(
            k,
            ErrorKind::Inconsistent {
                field: "generation",
                ..
            }
        )
    });
    assert_eq!(client.generation(), None);
}

#[test]
fn index_signed_by_an_unlisted_key_is_rejected() {
    let client = Client::new();
    let trust = client.accept_keys(&client.keys_v1()).expect("keys.json");
    let stranger = TestKey::generate();
    let err = assert_kind(
        client.accept(&trust, &stranger.sign_index(&index(T0))),
        |k| {
            matches!(k, ErrorKind::UnknownSigner { signer, trusted, .. }
            if *signer == stranger.id() && trusted.contains(&client.online.id()))
        },
    );
    assert!(
        err.to_string().contains(&stranger.id().to_string()),
        "{err}"
    );
}

#[test]
fn index_signed_by_a_revoked_key_is_rejected_even_with_a_higher_generation() {
    let client = Client::new();
    let trust = client.accept_keys(&client.keys_v1()).expect("keys.json");
    client
        .accept(&trust, &client.online.sign_index(&index(T0)))
        .expect("signed by a listed key");

    let next = TestKey::generate();
    let revoking = keys(
        2,
        vec![client.online.online(), next.online()],
        vec![client.online.id()],
    );
    let trust = client.accept_keys(&revoking).expect("newer keys.json");
    assert_kind(
        client.accept(&trust, &client.online.sign_index(&index(T0 + 100))),
        |k| matches!(k, ErrorKind::RevokedKey { signer } if *signer == client.online.id()),
    );
    client
        .accept(&trust, &next.sign_index(&index(T0 + 100)))
        .expect("signed by the new key");
}

#[test]
fn online_key_outside_its_window_is_rejected() {
    let client = Client::new();
    let mut late = client.online.online();
    late.not_before = Timestamp::from_secs(T0 + DAY);
    let trust = client
        .accept_keys(&keys(1, vec![late], vec![]))
        .expect("keys.json");
    assert_kind(
        client.accept(&trust, &client.online.sign_index(&index(T0))),
        |k| matches!(k, ErrorKind::KeyNotValid { .. }),
    );
}

#[test]
fn root_key_signs_indexes_in_direct_mode_and_in_emergencies() {
    let client = Client::new();
    client
        .accept(&client.direct(), &client.root.sign_index(&index(T0)))
        .expect("direct mode accepts the root key");
    let trust = client.accept_keys(&client.keys_v1()).expect("keys.json");
    client
        .accept(&trust, &client.root.sign_index(&index(T0 + 1)))
        .expect("a root key can always sign an index");
}

#[test]
fn online_key_is_rejected_in_direct_mode() {
    let client = Client::new();
    assert_kind(
        client.accept(&client.direct(), &client.online.sign_index(&index(T0))),
        |k| matches!(k, ErrorKind::UnknownSigner { .. }),
    );
}

#[test]
fn keys_json_that_disappears_after_acceptance_is_an_error() {
    let client = Client::new();
    client.accept_keys(&client.keys_v1()).expect("keys.json");
    let err = assert_kind(
        client.state.accept_keys(&client.repo, KeysFetch::NotFound),
        |k| matches!(k, ErrorKind::KeysMissing),
    );
    assert!(err.to_string().contains("keys.json is missing"), "{err}");
}

#[test]
fn changing_the_root_key_clears_the_keys_record() {
    let mut client = Client::new();
    client.accept_keys(&client.keys_v1()).expect("keys.json");
    client.root = TestKey::generate();
    client.repo = repo(&client.root);
    let trust = client.direct();
    assert!(trust.keys().is_none());
    assert_eq!(
        client
            .state
            .keys_record(&client.repo.name)
            .expect("readable"),
        None
    );
}

#[test]
fn older_keys_json_is_rejected() {
    let client = Client::new();
    let v2 = keys(2, vec![client.online.online()], vec![]);
    client.accept_keys(&v2).expect("keys.json");
    assert_kind(client.accept_keys(&client.keys_v1()), |k| {
        matches!(
            k,
            ErrorKind::KeysRollback {
                version: 1,
                accepted: 2
            }
        )
    });

    let mut other = v2.clone();
    other.revoked.push(TestKey::generate().id());
    assert_kind(client.accept_keys(&other), |k| {
        matches!(k, ErrorKind::KeysReplaced { version: 2 })
    });
    client
        .accept_keys(&v2)
        .expect("the accepted keys.json again");
}

#[test]
fn keys_json_not_signed_by_a_root_key_is_rejected() {
    let client = Client::new();
    let signed = client.online.sign_keys(&client.keys_v1());
    let fetch = KeysFetch::Found {
        json: &signed.bytes,
        signature: &signed.signature,
    };
    assert_kind(client.state.accept_keys(&client.repo, fetch), |k| {
        matches!(
            k,
            ErrorKind::UnknownSigner {
                document: "keys.json",
                ..
            }
        )
    });

    let mut signed = client.root.sign_keys(&client.keys_v1());
    signed.bytes.push(b' ');
    let fetch = KeysFetch::Found {
        json: &signed.bytes,
        signature: &signed.signature,
    };
    assert_kind(client.state.accept_keys(&client.repo, fetch), |k| {
        matches!(
            k,
            ErrorKind::BadSignature {
                document: "keys.json",
                ..
            }
        )
    });
    assert_eq!(
        client
            .state
            .keys_record(&client.repo.name)
            .expect("readable"),
        None
    );
}

#[test]
fn additional_root_key_signs_keys_json_while_the_root_is_replaced() {
    let mut client = Client::new();
    let next_root = TestKey::generate();
    client.repo.keys.push(next_root.public.clone());
    let signed = next_root.sign_keys(&client.keys_v1());
    let fetch = KeysFetch::Found {
        json: &signed.bytes,
        signature: &signed.signature,
    };
    let trust = client
        .state
        .accept_keys(&client.repo, fetch)
        .expect("signed by a root key");
    assert_eq!(trust.keys().map(|k| k.version), Some(1));
}

#[test]
fn rotating_to_the_additional_root_key_keeps_rollback_protection() {
    let mut client = Client::new();
    let next_root = TestKey::generate();
    client.repo.keys.push(next_root.public.clone());
    let accept = |client: &Client, signed: &Signed| {
        client.state.accept_keys(
            &client.repo,
            KeysFetch::Found {
                json: &signed.bytes,
                signature: &signed.signature,
            },
        )
    };
    let v1 = next_root.sign_keys(&client.keys_v1());
    let v2 = next_root.sign_keys(&keys(2, vec![client.online.online()], vec![]));
    accept(&client, &v2).expect("signed by the additional root key");

    client.repo = repo(&next_root);
    assert_kind(accept(&client, &v1), |k| {
        matches!(
            k,
            ErrorKind::KeysRollback {
                version: 1,
                accepted: 2
            }
        )
    });
    assert_kind(
        client.state.accept_keys(&client.repo, KeysFetch::NotFound),
        |k| matches!(k, ErrorKind::KeysMissing),
    );
    accept(&client, &v2).expect("the accepted keys.json again");
}

#[test]
fn generation_floor_recovers_a_client_frozen_by_a_leaked_key() {
    let client = Client::new();
    let trust = client.accept_keys(&client.keys_v1()).expect("keys.json");
    client
        .accept(&trust, &client.online.sign_index(&index(T0)))
        .expect("honest index");

    let inflated = T0 + 300 * DAY;
    let mut attack = index(inflated);
    attack.timestamp = Timestamp::from_secs(T0);
    attack.expires = Timestamp::from_secs(T0 + 14 * DAY);
    client
        .accept(&trust, &client.online.sign_index(&attack))
        .expect("the leaked key signs an inflated generation");

    let honest = index(T0 + 100);
    assert_kind(
        client.accept(&trust, &client.online.sign_index(&honest)),
        |k| matches!(k, ErrorKind::Rollback { .. }),
    );

    let next = TestKey::generate();

    let mut recovery = keys(2, vec![next.online()], vec![client.online.id()]);
    recovery.generation_floor = Some(T0);
    let trust = client.accept_keys(&recovery).expect("recovery keys.json");
    let record = &client
        .state
        .index_records(&client.repo.name)
        .expect("records")[0];
    assert_eq!((record.generation, record.blake3), (T0, None));

    client
        .accept(&trust, &next.sign_index(&honest))
        .expect("honest index above the floor");
    assert_eq!(client.generation(), Some(T0 + 100));
    assert_kind(
        client.accept(&trust, &next.sign_index(&index(T0 + 50))),
        |k| matches!(k, ErrorKind::Rollback { .. }),
    );
}

#[test]
fn generation_floor_never_raises_the_accepted_generation() {
    let client = Client::new();
    let trust = client.accept_keys(&client.keys_v1()).expect("keys.json");
    client
        .accept(&trust, &client.online.sign_index(&index(T0)))
        .expect("index");
    let before = client
        .state
        .index_records(&client.repo.name)
        .expect("records");

    let next = TestKey::generate();
    let mut revoke = keys(2, vec![next.online()], vec![client.online.id()]);
    revoke.generation_floor = Some(T0 + 100 * DAY);
    let trust = client.accept_keys(&revoke).expect("keys.json");
    assert_eq!(
        client
            .state
            .index_records(&client.repo.name)
            .expect("records"),
        before
    );
    client
        .accept(&trust, &next.sign_index(&index(T0 + 100)))
        .expect("an index between the accepted generation and the floor");
}

#[test]
fn reset_makes_the_next_sync_a_first_sync() {
    let client = Client::new();
    let trust = client.accept_keys(&client.keys_v1()).expect("keys.json");
    client
        .accept(&trust, &client.online.sign_index(&index(T0 + 100)))
        .expect("first index");

    client.state.reset(&client.repo.name).expect("reset");
    assert_eq!(
        client
            .state
            .keys_record(&client.repo.name)
            .expect("readable"),
        None
    );
    assert_eq!(client.generation(), None);
    let trust = client.direct();
    client
        .accept(&trust, &client.root.sign_index(&index(T0)))
        .expect("a lower generation after a reset");
}

#[test]
fn channels_keep_separate_histories() {
    let mut client = Client::new();
    let trust = client.direct();
    client
        .accept(&trust, &client.root.sign_index(&index(T0 + 100)))
        .expect("current");

    client.repo.channel = "testing".parse().expect("valid name");
    let mut testing = index(T0);
    testing.channel = client.repo.channel.clone();
    client
        .accept(&trust, &client.root.sign_index(&testing))
        .expect("testing has its own generation");

    let channels: Vec<_> = client
        .state
        .index_records(&client.repo.name)
        .expect("records")
        .into_iter()
        .map(|r| (r.channel.to_string(), r.generation))
        .collect();
    assert_eq!(
        channels,
        [("current".into(), T0 + 100), ("testing".into(), T0)]
    );
}

#[test]
fn leftover_temporary_record_does_not_block_writes() {
    let client = Client::new();
    let trust = client.direct();
    fs::create_dir_all(&client.state.dir).expect("state directory");
    fs::write(client.state.dir.join(".core.json.tmp"), b"partial").expect("leftover");
    client
        .accept(&trust, &client.root.sign_index(&index(T0)))
        .expect("accepted");
    assert_eq!(client.generation(), Some(T0));
}

#[test]
fn corrupt_record_is_an_error_rather_than_a_first_sync() {
    let client = Client::new();
    let trust = client.direct();
    client
        .accept(&trust, &client.root.sign_index(&index(T0 + 100)))
        .expect("first index");
    fs::write(client.state.dir.join("core.json"), b"{").expect("corrupt");
    assert_kind(
        client.accept(&trust, &client.root.sign_index(&index(T0))),
        |k| matches!(k, ErrorKind::CorruptState { .. }),
    );
}

#[test]
fn record_beyond_canonical_json_is_an_error_rather_than_a_panic() {
    let mut client = Client::new();
    let trust = client.direct();
    client.repo.channel = "testing".parse().expect("valid name");
    let mut testing = index(T0);
    testing.channel = client.repo.channel.clone();
    client
        .accept(&trust, &client.root.sign_index(&testing))
        .expect("testing index");
    let path = client.state.dir.join("core.json");
    let text = fs::read_to_string(&path).expect("readable");
    let huge = (1u64 << 60).to_string();
    fs::write(&path, text.replace(&T0.to_string(), &huge)).expect("tampered");

    client.repo.channel = "current".parse().expect("valid name");
    assert_kind(
        client.accept(&trust, &client.root.sign_index(&index(T0))),
        |k| matches!(k, ErrorKind::CorruptState { .. }),
    );
}
