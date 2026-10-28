use std::collections::BTreeSet;

use super::*;

const EXAMPLE: &str = r#"{"bad":[40],"events":[{"from":40,"kind":"fallback","time":"2026-10-26T08:01:12Z","to":39}],"good":[38,39,41],"last_good":41,"trial":{"gen":42,"tries_left":2},"version":1}"#;

#[test]
fn stored_state_round_trips_as_canonical_json() {
    let state = BootState::parse(EXAMPLE.as_bytes()).expect("valid");
    assert_eq!(state.trial.map(|t| t.tries_left), Some(2));
    assert_eq!(state.to_json().expect("canonical"), EXAMPLE.as_bytes());
}

#[test]
fn left_out_fields_take_their_empty_values() {
    let state = BootState::parse(br#"{"version":1}"#).expect("valid");
    assert_eq!(state, BootState::default());
}

#[test]
fn a_document_without_a_version_is_rejected() {
    assert!(matches!(
        BootState::parse(br#"{"bad":[1]}"#),
        Err(BootStateError::Invalid(_))
    ));
}

#[test]
fn another_version_is_reported_whatever_else_the_document_holds() {
    let err = BootState::parse(br#"{"version":2,"attempts":{"gen":"x"}}"#).unwrap_err();
    assert_eq!(err.to_string(), "unsupported version 2");
}

#[test]
fn unknown_fields_of_this_version_are_rejected() {
    let unknown = EXAMPLE.replace(r#""version":1"#, r#""version":1,"extra":0"#);
    assert!(BootState::parse(unknown.as_bytes()).is_err());
    let in_trial = EXAMPLE.replace(r#""tries_left":2"#, r#""tries_left":2,"extra":0"#);
    assert!(BootState::parse(in_trial.as_bytes()).is_err());
}

#[test]
fn pruning_forgets_generations_that_are_gone() {
    let mut state = BootState::parse(EXAMPLE.as_bytes()).expect("valid");
    state.prune(&BTreeSet::from([39, 41, 42]));
    assert_eq!(state.good, BTreeSet::from([39, 41]));
    assert!(state.bad.is_empty());
    assert_eq!(state.last_good, Some(41));
}

#[test]
fn only_the_most_recent_events_are_kept() {
    let mut state = BootState::default();
    for n in 0..=MAX_BOOT_EVENTS as u64 {
        state.push_event(BootEvent {
            from: n,
            kind: BootEventKind::Fallback,
            time: Timestamp::from_secs(n),
            to: n + 1,
        });
    }
    assert_eq!(state.events.len(), MAX_BOOT_EVENTS);
    assert_eq!(state.events[0].from, 1);
    assert_eq!(
        state.events[MAX_BOOT_EVENTS - 1].from,
        MAX_BOOT_EVENTS as u64
    );
}
