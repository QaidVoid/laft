use std::path::Path;

use super::*;

#[test]
fn a_missing_new_root_fails_before_the_point_of_no_return() {
    if Path::new(NEWROOT).exists() {
        println!("skipping: this machine has a {NEWROOT}");
        return;
    }
    let cwd = std::env::current_dir().unwrap();
    let err = prepare_switch_root().unwrap_err();
    assert!(err.contains("cannot enter the new root"), "{err}");
    assert_eq!(std::env::current_dir().unwrap(), cwd);
}
