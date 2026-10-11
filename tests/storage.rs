use bg_key_dbclk_checker::{
    detector::Range,
    event::{CaptureMessage, Key, KeyAction, KeyEvent, Source},
    record::{LogRecord, PressRecord, RecordKind},
    session::Session,
    storage::{self, LogStore},
};
use chrono::{TimeZone, Utc};
use std::{fs, path::Path};

fn event(action: KeyAction, ms: u64) -> KeyEvent {
    KeyEvent {
        source: Source {
            id: "keyboard".into(),
            name: Some("Test keyboard".into()),
        },
        key: Key {
            code: 30,
            name: "A".into(),
        },
        action,
        monotonic_ns: ms * 1_000_000,
        timestamp: Utc
            .timestamp_opt(
                1_700_000_000 + ms as i64 / 1000,
                (ms % 1000) as u32 * 1_000_000,
            )
            .unwrap(),
    }
}
fn record(id: &str) -> LogRecord {
    LogRecord::Keystroke(PressRecord::new(&event(KeyAction::Press, 100), id))
}
fn session(path: &Path, all_keys: bool) -> Session {
    Session::new(
        Range::new(0.0, 80.0).unwrap(),
        all_keys,
        LogStore::open(path).unwrap(),
        "session".into(),
    )
}
fn feed(s: &mut Session, action: KeyAction, ms: u64) {
    s.handle(CaptureMessage::Event(event(action, ms))).unwrap();
}

#[test]
fn append_reopen_preserves_records_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/events.jsonl");
    LogStore::open(&path)
        .unwrap()
        .append(&record("first"))
        .unwrap();
    let first = fs::read(&path).unwrap();
    LogStore::open(&path)
        .unwrap()
        .append(&record("second"))
        .unwrap();
    assert!(fs::read(&path).unwrap().starts_with(&first));
    let rows = storage::view(&path, RecordKind::All, 100).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].record.press().session_id, "first");
    let json: serde_json::Value = serde_json::from_str(&rows[0].json).unwrap();
    for field in [
        "record_type",
        "schema_version",
        "timestamp",
        "key",
        "source",
        "platform",
        "session_id",
    ] {
        assert!(!json[field].is_null(), "missing {field}");
    }
}

#[test]
fn optional_all_keys_uses_the_same_physical_press_filter() {
    for all_keys in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let mut s = session(&path, all_keys);
        s.handle(CaptureMessage::Reset {
            source: event(KeyAction::Press, 0).source,
            held_keys: vec![event(KeyAction::Press, 0).key],
        })
        .unwrap();
        feed(&mut s, KeyAction::Press, 10); // held before startup
        feed(&mut s, KeyAction::Repeat, 20);
        feed(&mut s, KeyAction::Release, 30);
        feed(&mut s, KeyAction::Press, 100);
        feed(&mut s, KeyAction::Repeat, 110);
        feed(&mut s, KeyAction::Press, 120); // duplicate down while held
        feed(&mut s, KeyAction::Release, 130);
        feed(&mut s, KeyAction::Press, 150);
        assert_eq!(
            storage::view(&path, RecordKind::Keystroke, 100)
                .unwrap()
                .len(),
            if all_keys { 2 } else { 0 }
        );
        let rows = storage::view(&path, RecordKind::Symptom, 100).unwrap();
        assert_eq!(rows.len(), 1);
        let LogRecord::Symptom(symptom) = &rows[0].record else {
            panic!("wrong type")
        };
        assert_eq!(symptom.interval_ms, 50.0);
        drop(s);
        let mut later = session(&path, false);
        feed(&mut later, KeyAction::Press, 200);
        assert_eq!(
            storage::view(&path, RecordKind::All, 100).unwrap().len(),
            if all_keys { 3 } else { 1 }
        );
    }
}

#[test]
fn filtering_precedes_latest_limit_and_preserves_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let mut s = session(&path, true);
    for ms in [100, 130, 160] {
        feed(&mut s, KeyAction::Release, ms - 1);
        feed(&mut s, KeyAction::Press, ms);
    }
    let rows = storage::view(&path, RecordKind::Keystroke, 2).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].record.press().timestamp,
        event(KeyAction::Press, 130).timestamp
    );
    assert_eq!(
        rows[1].record.press().timestamp,
        event(KeyAction::Press, 160).timestamp
    );
    assert_eq!(
        storage::view(&path, RecordKind::Symptom, 100)
            .unwrap()
            .len(),
        2
    );
    assert!(storage::view(&path, RecordKind::All, 0).is_err());
}

#[test]
fn clear_requires_confirmation_and_open_writer_can_append_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let writer = LogStore::open(&path).unwrap();
    writer.append(&record("old")).unwrap();
    assert!(storage::clear(&path, false).is_err());
    assert_eq!(storage::view(&path, RecordKind::All, 100).unwrap().len(), 1);
    assert!(storage::clear(&path, true).unwrap());
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    assert!(!storage::clear(&path, true).unwrap());
    writer.append(&record("new")).unwrap();
    assert_eq!(
        storage::view(&path, RecordKind::All, 100).unwrap()[0]
            .record
            .press()
            .session_id,
        "new"
    );
}

#[test]
fn absent_logs_do_not_create_files_and_invalid_paths_fail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent/log");
    assert!(storage::view(&path, RecordKind::All, 100)
        .unwrap()
        .is_empty());
    assert!(!storage::clear(&path, true).unwrap());
    assert!(!path.exists());
    assert!(!path.parent().unwrap().exists());
    assert!(LogStore::open(dir.path()).is_err());
}

#[test]
fn malformed_unknown_schema_and_incomplete_tail_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    fs::write(&path, b"not json\n").unwrap();
    assert!(storage::view(&path, RecordKind::All, 100)
        .err()
        .unwrap()
        .to_string()
        .contains("line 1"));
    let mut value = serde_json::to_value(record("test")).unwrap();
    value["schema_version"] = 999.into();
    fs::write(&path, format!("{value}\n")).unwrap();
    assert!(storage::view(&path, RecordKind::All, 100)
        .err()
        .unwrap()
        .to_string()
        .contains("schema"));
    fs::write(&path, b"{\"partial\":").unwrap();
    assert!(LogStore::open(&path).is_err());
    storage::clear(&path, true).unwrap();
    LogStore::open(&path)
        .unwrap()
        .append(&record("recovered"))
        .unwrap();
}

#[test]
fn concurrent_writers_and_readers_keep_complete_json_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    LogStore::open(&path).unwrap();
    std::thread::scope(|scope| {
        for worker in 0..3 {
            let path = &path;
            scope.spawn(move || {
                let writer = LogStore::open(path).unwrap();
                for i in 0..20 {
                    writer.append(&record(&format!("{worker}-{i}"))).unwrap();
                }
            });
        }
        let path = &path;
        scope.spawn(move || {
            for _ in 0..20 {
                for row in storage::view(path, RecordKind::All, 100).unwrap() {
                    serde_json::from_str::<LogRecord>(&row.json).unwrap();
                }
            }
        });
    });
    let rows = storage::view(&path, RecordKind::All, 100).unwrap();
    assert_eq!(rows.len(), 60);
    let ids: std::collections::HashSet<_> =
        rows.iter().map(|r| &r.record.press().session_id).collect();
    assert_eq!(ids.len(), 60);
}

#[test]
fn interrupted_tail_after_open_prevents_a_successful_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let writer = LogStore::open(&path).unwrap();
    std::fs::write(&path, b"unfinished").unwrap();
    assert!(writer.append(&record("not-saved")).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"unfinished");
}
