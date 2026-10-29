use super::*;

#[test]
fn known_codes_explain_the_problem_and_the_fix_in_any_case() {
    let text = explain("c0901").expect("a configuration code");
    assert!(text.starts_with("C0901: "), "{text}");
    assert!(text.contains("fix: "), "{text}");
    assert!(explain("R003").is_some());
    assert!(explain("KR011").is_some());
    assert!(explain("Z9999").is_none());
}

#[test]
fn every_code_is_listed_once() {
    let mut codes: Vec<&str> = TEXTS.iter().map(|(c, _, _)| *c).collect();
    let all = codes.len();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all);
}

#[test]
fn every_account_code_of_activation_is_explained() {
    for code in ["C0409", "C0410", "C0411", "C0412", "C0413"] {
        assert!(explain(code).is_some(), "{code}");
    }
}
