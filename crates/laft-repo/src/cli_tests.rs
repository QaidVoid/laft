use super::*;

#[test]
fn durations_take_a_unit() {
    assert_eq!(parse_duration("90s"), Ok(90));
    assert_eq!(parse_duration("30m"), Ok(1800));
    assert_eq!(parse_duration("24h"), Ok(86_400));
    assert_eq!(parse_duration("14d"), Ok(14 * 86_400));
    for bad in ["", "14", "d", "1.5d", "-1d", "14w", "99999999999999999999d"] {
        assert!(parse_duration(bad).is_err(), "{bad}");
    }
}

#[test]
fn defaults_are_given_as_key_and_name() {
    let (group, name) = parse_pair::<Name>("vi=neovim").unwrap();
    assert_eq!((group.as_str(), name.as_str()), ("vi", "neovim"));
    let (provide, name) = parse_pair::<String>("so:libjpeg.so.8=libjpeg-turbo").unwrap();
    assert_eq!(
        (provide.as_str(), name.as_str()),
        ("so:libjpeg.so.8", "libjpeg-turbo")
    );
    assert!(parse_pair::<Name>("vi").is_err());
    assert!(parse_pair::<Name>("vi=Not A Name").is_err());
}

#[test]
fn the_command_line_is_consistent() {
    use clap::CommandFactory;
    Cli::command().debug_assert();
}
