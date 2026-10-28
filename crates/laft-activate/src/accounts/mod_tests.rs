use super::*;

#[test]
fn names_follow_the_account_name_grammar() {
    for name in ["alice", "_svc", "a1-b_c", &"a".repeat(32)] {
        assert!(valid_name(name), "{name:?}");
    }
    for name in ["", "Alice", "1abc", "-x", "a:b", "a b", &"a".repeat(33)] {
        assert!(!valid_name(name), "{name:?}");
    }
}

#[test]
fn hashes_must_be_supported_and_storable() {
    for hash in ["!", "*", "$6$salt$abc", "!$y$j9T$abc", "$2b$10$x", "$gy$a"] {
        assert!(valid_hash(hash), "{hash:?}");
    }
    for hash in ["", "plain", "$1$md5$x", "$6$", "$6$a:b", "$6$a\nb", "!!"] {
        assert!(!valid_hash(hash), "{hash:?}");
    }
}
