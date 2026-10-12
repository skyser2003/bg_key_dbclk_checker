use bg_key_dbclk_checker::{
    detector::Range,
    event::{CaptureMessage, Key, KeyAction, KeyEvent, Source},
    session::Session,
    storage::LogStore,
};
use std::{fs, path::Path};

fn session(path: &Path, all_keys: bool, id: &str) -> Session {
    Session::new(
        Range::new(0.0, 80.0).unwrap(),
        all_keys,
        LogStore::open(path).unwrap(),
        id.into(),
    )
}

fn event(source: &str, key: &str, code: u32, action: KeyAction, ms: u64) -> CaptureMessage {
    CaptureMessage::Event(KeyEvent {
        source: Source {
            id: source.into(),
            name: None,
        },
        key: Key {
            code,
            name: key.into(),
        },
        action,
        monotonic_ns: ms * 1_000_000,
        timestamp: chrono::Utc::now(),
    })
}

fn presses(session: &mut Session, source: &str, key: &str, code: u32, times: &[u64]) {
    for &ms in times {
        session
            .handle(event(source, key, code, KeyAction::Release, ms - 1))
            .unwrap();
        session
            .handle(event(source, key, code, KeyAction::Press, ms))
            .unwrap();
    }
}

fn summary(session: &Session) -> String {
    let mut output = Vec::new();
    session.write_summary(&mut output).unwrap();
    String::from_utf8(output).unwrap()
}

#[test]
fn summary_counts_each_native_key_and_keyboard_most_frequent_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session(&dir.path().join("log"), false, "current");
    presses(&mut session, "keyboard-a", "B", 48, &[100, 130]);
    presses(&mut session, "keyboard-b", "A", 30, &[200, 230]);
    presses(&mut session, "keyboard-a", "A", 30, &[300, 330, 360]);
    let output = summary(&session);
    assert!(output.contains("Double-click summary (current session)"));
    assert!(output.contains("Total symptoms: 4"));
    let most = output
        .find("2 key=\"A\" code=30 source=\"keyboard-a\"")
        .unwrap();
    let next = output
        .find("1 key=\"A\" code=30 source=\"keyboard-b\"")
        .unwrap();
    let last = output
        .find("1 key=\"B\" code=48 source=\"keyboard-a\"")
        .unwrap();
    assert!(most < next && next < last);
}

#[test]
fn summary_uses_only_saved_symptoms_from_the_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let mut old = session(&path, false, "old");
    presses(&mut old, "keyboard", "B", 48, &[100, 130, 160]);
    drop(old);
    let mut current = session(&path, true, "current");
    current
        .handle(CaptureMessage::Reset {
            source: Source {
                id: "keyboard".into(),
                name: None,
            },
            held_keys: vec![Key {
                code: 30,
                name: "A".into(),
            }],
        })
        .unwrap();
    for (action, ms) in [
        (KeyAction::Press, 10), // Already held at startup.
        (KeyAction::Repeat, 20),
        (KeyAction::Release, 30),
        (KeyAction::Press, 100),
        (KeyAction::Repeat, 110),
        (KeyAction::Press, 120), // Duplicate down while held.
        (KeyAction::Release, 130),
        (KeyAction::Press, 150),
    ] {
        current
            .handle(event("keyboard", "A", 30, action, ms))
            .unwrap();
    }
    presses(&mut current, "keyboard", "C", 46, &[200]);
    let output = summary(&current);
    assert!(output.contains("Total symptoms: 1"));
    assert!(output.contains("1 key=\"A\" code=30 source=\"keyboard\""));
    assert!(!output.contains("key=\"B\""));
    assert!(!output.contains("key=\"C\""));
}

#[test]
fn empty_summary_explicitly_reports_no_symptoms() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session(&dir.path().join("log"), true, "current");
    presses(&mut session, "keyboard", "A", 30, &[100]);
    let output = summary(&session);
    assert!(output.contains("Total symptoms: 0"));
    assert!(output.contains("No double-click symptoms detected."));
    assert!(!output.contains("key="));
}

#[test]
fn failed_save_is_not_included_in_summary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let mut session = session(&path, false, "current");
    presses(&mut session, "keyboard", "A", 30, &[100]);
    session
        .handle(event("keyboard", "A", 30, KeyAction::Release, 110))
        .unwrap();
    fs::write(&path, "unfinished record").unwrap();
    assert!(session
        .handle(event("keyboard", "A", 30, KeyAction::Press, 130))
        .is_err());
    assert!(summary(&session).contains("Total symptoms: 0"));
}
