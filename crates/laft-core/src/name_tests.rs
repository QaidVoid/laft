use super::*;

#[test]
fn accepts_names_with_digits_dots_and_dashes() {
    for ok in [
        "zlib",
        "openssl1.1",
        "python3.12",
        "xdg-utils",
        "0ad",
        "lib_foo",
        "gtk3",
    ] {
        assert!(ok.parse::<Name>().is_ok(), "{ok} should be a valid name");
    }
}

#[test]
fn rejects_names_outside_the_alphabet() {
    for bad in [
        "",
        "Foo",
        "gtk+3",
        "foo.bar",
        "foo.",
        "-foo",
        "llvm@19",
        "rg:static",
        "a b",
    ] {
        assert!(bad.parse::<Name>().is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn rejects_names_longer_than_64_bytes() {
    assert!("a".repeat(64).parse::<Name>().is_ok());
    assert!("a".repeat(65).parse::<Name>().is_err());
}

#[test]
fn error_message_names_the_problem() {
    let err = "Foo".parse::<Name>().expect_err("uppercase is rejected");
    assert_eq!(
        err.to_string(),
        "invalid package name \"Foo\": a name starts with a lowercase letter or a digit"
    );
}
