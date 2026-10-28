use super::Cmdline;

#[test]
fn laft_parameters_are_collected_and_others_skipped() {
    let cmdline = Cmdline::parse(
        "BOOT_IMAGE=/vmlinuz laft.gen=42 quiet loglevel=3 laft.break=premount \
         laft.break=preswitch laft.init=/usr/bin/sh laft.notries laft.rootwait=90 laft.debug\n",
    );
    assert_eq!(
        cmdline,
        Cmdline {
            generation: Some(42),
            break_premount: true,
            break_preswitch: true,
            init: Some("/usr/bin/sh".into()),
            notries: true,
            rootwait: Some(90),
            debug: true,
            warnings: vec![],
        }
    );
}

#[test]
fn empty_command_line_selects_nothing() {
    assert_eq!(Cmdline::parse(""), Cmdline::default());
}

#[test]
fn last_occurrence_wins() {
    let cmdline = Cmdline::parse("laft.gen=41 laft.gen=42");
    assert_eq!(cmdline.generation, Some(42));
}

#[test]
fn quotes_keep_spaces_inside_one_parameter() {
    let cmdline = Cmdline::parse(r#"foo="a laft.gen=7 b" laft.init="/usr/bin/my init""#);
    assert_eq!(cmdline.generation, None);
    assert_eq!(cmdline.init.as_deref(), Some("/usr/bin/my init"));
}

#[test]
fn unknown_and_malformed_parameters_are_ignored_with_a_warning() {
    let cmdline = Cmdline::parse(
        "laft.gen=+42 laft.gen=4x laft.break=later laft.init=sh laft.rootwait= \
         laft.notries=1 laft.debug=yes laft.frobnicate",
    );
    assert_eq!(cmdline.generation, None);
    assert!(!cmdline.break_premount && !cmdline.break_preswitch);
    assert_eq!(cmdline.init, None);
    assert_eq!(cmdline.rootwait, None);
    assert!(!cmdline.notries && !cmdline.debug);
    assert_eq!(cmdline.warnings.len(), 8, "{:?}", cmdline.warnings);
    assert!(cmdline.warnings[7].contains("laft.frobnicate"));
}

#[test]
fn a_bad_value_keeps_an_earlier_good_one() {
    let cmdline = Cmdline::parse("laft.gen=42 laft.gen=abc");
    assert_eq!(cmdline.generation, Some(42));
    assert_eq!(cmdline.warnings.len(), 1);
}
