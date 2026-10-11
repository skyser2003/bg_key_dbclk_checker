use bg_key_dbclk_checker::{
    event::{Key, KeyAction, KeyEvent, Source},
    record::{LogRecord, PressRecord},
};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bg-key-dbclk-checker"))
        .arg("--output")
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
fn seed(path: &Path) {
    let event = KeyEvent {
        source: Source {
            id: "keyboard".into(),
            name: Some("Keyboard".into()),
        },
        key: Key {
            code: 30,
            name: "A".into(),
        },
        action: KeyAction::Press,
        monotonic_ns: 100,
        timestamp: chrono::Utc::now(),
    };
    let record = LogRecord::Keystroke(PressRecord::new(&event, "fixture"));
    fs::write(
        path,
        format!("{}\n", serde_json::to_string(&record).unwrap()),
    )
    .unwrap();
}

#[test]
fn help_describes_monitor_and_log_commands() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let output = run(&path, &["--help"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("monitor"));
    assert!(text.contains("logs"));
    let output = run(&path, &["monitor", "--help"]);
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("--all-keys"));
}

#[test]
fn view_and_clear_work_without_capture_and_preserve_records_without_yes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    seed(&path);
    let original = fs::read(&path).unwrap();
    let output = run(&path, &["logs", "view", "--kind", "keystroke", "--json"]);
    assert!(output.status.success(), "{:?}", output);
    let row: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["record_type"], "keystroke");
    assert_eq!(row["session_id"], "fixture");
    assert!(!run(&path, &["logs", "clear"]).status.success());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(run(&path, &["logs", "clear", "--yes"]).status.success());
    assert!(fs::read(&path).unwrap().is_empty());
}

#[test]
fn empty_queries_are_parseable_and_do_not_create_logs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing/log");
    let output = run(&path, &["logs", "view", "--json"]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!path.exists());
    assert!(run(&path, &["logs", "clear", "--yes"]).status.success());
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn invalid_ranges_and_limits_fail_before_capture() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    for args in [
        &["monitor", "--min-ms", "81", "--max-ms", "80"][..],
        &["monitor", "--max-ms", "0"],
        &["monitor", "--min-ms", "NaN"],
        &["monitor", "--max-ms", "inf"],
        &["logs", "view", "--limit", "0"],
    ] {
        let output = run(&path, args);
        assert!(!output.status.success(), "arguments {args:?}");
        assert!(!path.exists());
    }
}

#[test]
fn malformed_logs_report_line_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    fs::write(&path, "bad json\n").unwrap();
    let output = run(&path, &["logs", "view"]);
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("line 1"));
}

#[cfg(target_os = "linux")]
#[test]
fn default_data_path_is_shared_by_view_and_clear() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bg-key-dbclk-checker/symptoms.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    seed(&path);
    let output = Command::new(env!("CARGO_BIN_EXE_bg-key-dbclk-checker"))
        .env("XDG_DATA_HOME", dir.path())
        .args(["logs", "view", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
    let output = Command::new(env!("CARGO_BIN_EXE_bg-key-dbclk-checker"))
        .env("XDG_DATA_HOME", dir.path())
        .args(["logs", "clear", "--yes"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(fs::read(&path).unwrap().is_empty());
}
