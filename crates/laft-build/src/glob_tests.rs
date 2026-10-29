use super::*;

#[test]
fn stars_stay_within_one_segment() {
    let glob = Glob::new("usr/lib/*.so.*");
    assert!(glob.matches("usr/lib/libssl.so.3"));
    assert!(!glob.matches("usr/lib/engines/libx.so.3"));
    assert!(!glob.matches("usr/lib/libssl.so"));
}

#[test]
fn double_stars_match_any_number_of_segments() {
    let glob = Glob::new("usr/**/*.h");
    assert!(glob.matches("usr/x.h"));
    assert!(glob.matches("usr/include/a/b/x.h"));
    assert!(!glob.matches("etc/x.h"));
}

#[test]
fn question_marks_match_one_character() {
    let glob = Glob::new("usr/bin/?q");
    assert!(glob.matches("usr/bin/jq"));
    assert!(!glob.matches("usr/bin/q"));
    assert!(!glob.matches("usr/bin/jjq"));
}

#[test]
fn braces_match_either_alternative() {
    let glob = Glob::new("usr/share/{man,info}/**");
    assert!(glob.matches("usr/share/man/man1/x.1"));
    assert!(glob.matches("usr/share/info/x.info"));
    assert!(!glob.matches("usr/share/doc/x"));
}

#[test]
fn a_glob_matching_a_directory_matches_its_contents() {
    let glob = Glob::new("etc/ssl");
    assert!(glob.matches("etc/ssl"));
    assert!(glob.matches("etc/ssl/certs/ca.pem"));
    assert!(!glob.matches("etc/ssl2"));
}
