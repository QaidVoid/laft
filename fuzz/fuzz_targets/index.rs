//! Feeds arbitrary bytes to the parsers of repository documents: indexes,
//! `keys.json`, minisign signatures, public keys and trusted comments.
//!
//! The first input byte picks the parser. Documents that parse must
//! serialize again and parse back to the same value.

#![no_main]

use laft_index::{Index, IndexComment, KeyId, Keys, KeysComment, PublicKey, Signature};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&parser, rest)) = data.split_first() else {
        return;
    };
    match parser % 5 {
        0 => {
            let _ = Index::decode(rest);
        }
        1 => {
            if let Ok(index) = Index::from_json(rest) {
                let json = index.to_json().expect("a parsed index is valid");
                assert_eq!(Index::from_json(&json).expect("written JSON parses"), index);
            }
        }
        2 => {
            if let Ok(keys) = Keys::from_json(rest) {
                let json = keys.to_json().expect("a parsed keys.json is valid");
                assert_eq!(Keys::from_json(&json).expect("written JSON parses"), keys);
            }
        }
        3 => {
            if let Ok(signature) = Signature::decode(rest) {
                let _ = signature.trusted_comment().parse::<IndexComment>();
                let _ = signature.trusted_comment().parse::<KeysComment>();
            }
        }
        _ => {
            let Ok(text) = std::str::from_utf8(rest) else {
                return;
            };
            if let Ok(comment) = text.parse::<IndexComment>() {
                assert_eq!(comment.to_string(), text);
            }
            if let Ok(comment) = text.parse::<KeysComment>() {
                assert_eq!(comment.to_string(), text);
            }
            if let Ok(key) = text.parse::<PublicKey>() {
                assert_eq!(key.to_string(), text);
            }
            if let Ok(id) = text.parse::<KeyId>() {
                assert_eq!(id.to_string(), text);
            }
        }
    }
});
