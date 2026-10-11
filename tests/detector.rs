use bg_key_dbclk_checker::{
    detector::{Detector, Range},
    event::{Key, KeyAction, KeyEvent, Source},
};
use chrono::{TimeZone, Utc};

fn source(id: &str) -> Source {
    Source {
        id: id.into(),
        name: Some(id.into()),
    }
}
fn event(id: &str, code: u32, action: KeyAction, ms: f64) -> KeyEvent {
    KeyEvent {
        source: source(id),
        key: Key {
            code,
            name: format!("Key{code}"),
        },
        action,
        monotonic_ns: (ms * 1_000_000.0) as u64,
        timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}
fn press(
    d: &mut Detector,
    id: &str,
    code: u32,
    ms: f64,
) -> bg_key_dbclk_checker::detector::Detection {
    d.process(event(id, code, KeyAction::Release, ms - 0.001));
    d.process(event(id, code, KeyAction::Press, ms))
}

#[test]
fn inclusive_fractional_bounds_and_outside_intervals() {
    for (interval, matches) in [
        (9.999, false),
        (10.0, true),
        (10.5, true),
        (80.0, true),
        (80.001, false),
    ] {
        let mut d = Detector::new(Range::new(10.0, 80.0).unwrap());
        assert!(press(&mut d, "one", 30, 100.0).accepted_press.is_some());
        let result = press(&mut d, "one", 30, 100.0 + interval);
        assert_eq!(result.symptom.is_some(), matches, "interval {interval}");
        if let Some(s) = result.symptom {
            assert!((s.interval_ms - interval).abs() < 0.00001);
        }
    }
}

#[test]
fn repeats_and_duplicate_downs_never_count_or_replace_history() {
    let mut d = Detector::new(Range::new(0.0, 80.0).unwrap());
    assert!(press(&mut d, "one", 30, 100.0).accepted_press.is_some());
    for (action, ms) in [
        (KeyAction::Repeat, 130.0),
        (KeyAction::Press, 150.0),
        (KeyAction::Repeat, 175.0),
    ] {
        let result = d.process(event("one", 30, action, ms));
        assert!(result.accepted_press.is_none());
        assert!(result.symptom.is_none());
    }
    assert!(press(&mut d, "one", 30, 200.0).symptom.is_none());
    assert!(press(&mut d, "one", 30, 230.0).symptom.is_some());
}

#[test]
fn interleaved_keys_and_devices_have_independent_history() {
    let mut d = Detector::new(Range::new(0.0, 80.0).unwrap());
    press(&mut d, "one", 30, 100.0);
    press(&mut d, "one", 31, 120.0);
    assert!(press(&mut d, "two", 30, 130.0).symptom.is_none());
    assert!(press(&mut d, "one", 30, 140.0).symptom.is_some());
}

#[test]
fn rapid_triples_form_two_adjacent_pairs_despite_wall_clock_changes() {
    let mut d = Detector::new(Range::new(0.0, 80.0).unwrap());
    press(&mut d, "one", 30, 100.0);
    let second = press(&mut d, "one", 30, 120.0).symptom.unwrap();
    d.process(event("one", 30, KeyAction::Release, 125.0));
    let mut third_event = event("one", 30, KeyAction::Press, 150.0);
    third_event.timestamp = Utc.timestamp_opt(100, 0).unwrap();
    let third = d.process(third_event).symptom.unwrap();
    assert_eq!(second.interval_ms, 20.0);
    assert_eq!(third.interval_ms, 30.0);
    assert_eq!(third.previous.monotonic_ns, 120_000_000);
}

#[test]
fn reset_forgets_history_and_seeds_startup_held_keys() {
    let mut d = Detector::new(Range::new(0.0, 80.0).unwrap());
    press(&mut d, "one", 30, 100.0);
    d.reset_source(
        &source("one"),
        vec![Key {
            code: 30,
            name: "A".into(),
        }],
    );
    assert!(d
        .process(event("one", 30, KeyAction::Press, 110.0))
        .accepted_press
        .is_none());
    assert!(press(&mut d, "one", 30, 120.0).symptom.is_none());
    assert!(press(&mut d, "one", 30, 140.0).symptom.is_some());
    d.reset_source(&source("one"), vec![]);
    assert!(press(&mut d, "one", 30, 150.0).symptom.is_none());
}

#[test]
fn invalid_ranges_are_rejected() {
    for (min, max) in [
        (-1.0, 80.0),
        (0.0, 0.0),
        (81.0, 80.0),
        (f64::NAN, 80.0),
        (0.0, f64::INFINITY),
    ] {
        assert!(Range::new(min, max).is_err());
    }
}

#[test]
fn backwards_monotonic_event_never_forms_a_pair() {
    let mut d = Detector::new(Range::new(0.0, 80.0).unwrap());
    press(&mut d, "one", 30, 100.0);
    assert!(press(&mut d, "one", 30, 90.0).symptom.is_none());
}
