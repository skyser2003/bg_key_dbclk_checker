use bg_key_dbclk_checker::{
    detector::{Detector, Range},
    event::{Key, KeyAction, KeyEvent, Source},
    record::{LogRecord, PressRecord, SymptomRecord},
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

struct SymptomFixture;

impl SymptomFixture {
    fn write(path: &Path, name: Option<&str>) {
        let mut detector = Detector::new(Range::new(0.0, 80.0).unwrap());
        for (action, monotonic_ns) in [
            (KeyAction::Press, 0),
            (KeyAction::Release, 10_000_000),
            (KeyAction::Press, 30_750_000),
        ] {
            let result = detector.process(KeyEvent {
                source: Source {
                    id: "system-keyboard-uid".into(),
                    name: name.map(str::to_owned),
                },
                key: Key {
                    code: 30,
                    name: "A".into(),
                },
                action,
                monotonic_ns,
                timestamp: chrono::DateTime::parse_from_rfc3339("2026-10-30T16:34:56.987654321Z")
                    .unwrap()
                    .to_utc(),
            });
            if let Some(symptom) = result.symptom {
                fs::write(
                    path,
                    format!(
                        "{}\n",
                        serde_json::to_string(&LogRecord::Symptom(SymptomRecord::new(
                            &symptom, "fixture"
                        )))
                        .unwrap()
                    ),
                )
                .unwrap();
            }
        }
        assert!(path.exists());
    }

    fn view(path: &Path, args: &[&str], color: bool) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bg-key-dbclk-checker"))
            .arg("--output")
            .arg(path)
            .args(["logs", "view"])
            .args(args)
            .env_remove("NO_COLOR")
            .env("CLICOLOR_FORCE", if color { "1" } else { "" })
            .env("TERM", "xterm-256color")
            .output()
            .unwrap()
    }
}

#[test]
fn readable_logs_show_keyboard_names_and_whole_millisecond_delays() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    SymptomFixture::write(&path, Some("Mechanical Keyboard"));
    let output = SymptomFixture::view(&path, &[], false);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Mechanical Keyboard"), "{text}");
    assert!(text.contains("31 ms"), "{text}");
    assert!(!text.contains("system-keyboard-uid"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn readable_logs_fall_back_to_system_uid_without_a_keyboard_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    for name in [None, Some(""), Some("   ")] {
        SymptomFixture::write(&path, name);
        let output = SymptomFixture::view(&path, &[], false);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("system-keyboard-uid"), "{text}");
    }
}

#[test]
fn readable_logs_color_timestamps_keys_and_delays_when_color_is_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    SymptomFixture::write(&path, Some("Keyboard"));
    let output = SymptomFixture::view(&path, &[], true);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let timestamp = text
        .lines()
        .find(|line| line.contains("31 ms"))
        .unwrap()
        .split('|')
        .nth(1)
        .unwrap()
        .trim();
    assert!(timestamp.starts_with("\u{1b}[32m"), "{text:?}");
    assert!(timestamp.ends_with("\u{1b}[0m"), "{text:?}");
    assert!(text.contains("\u{1b}[36m\"A\"\u{1b}[0m"), "{text:?}");
    assert!(text.contains("\u{1b}[33m31 ms\u{1b}[0m"), "{text:?}");
}

#[test]
fn readable_logs_use_aligned_columns_and_second_precision_local_times() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    SymptomFixture::write(&path, Some("Mechanical Keyboard"));
    let output = SymptomFixture::view(&path, &[], false);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let header = text.lines().find(|line| line.contains("Keyboard")).unwrap();
    for column in ["Time (local)", "Type", "Key", "Code", "Delay", "Keyboard"] {
        assert!(header.contains(column), "{text}");
    }
    let row = text.lines().find(|line| line.contains("31 ms")).unwrap();
    let columns: Vec<&str> = row.split('|').map(str::trim).collect();
    assert_eq!(columns.len(), 8, "{text}");
    assert!(chrono::NaiveDateTime::parse_from_str(columns[1], "%Y-%m-%d %H:%M:%S").is_ok());
    assert_eq!(columns[1].len(), 19);
    assert_eq!(columns[2], "Symptom");
    assert_eq!(columns[3], "\"A\"");
    assert_eq!(columns[4], "30");
    assert_eq!(columns[5], "31 ms");
    assert_eq!(columns[6], "\"Mechanical Keyboard\"");
    assert!(!text.contains(".987654321"));
    assert!(!text.contains("key="));
}

#[cfg(unix)]
#[test]
fn readable_log_timestamps_follow_the_local_timezone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    SymptomFixture::write(&path, Some("Keyboard"));
    let output = Command::new(env!("CARGO_BIN_EXE_bg-key-dbclk-checker"))
        .arg("--output")
        .arg(&path)
        .args(["logs", "view"])
        .env("TZ", "America/New_York")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("2026-10-30 12:34:56"), "{text}");
    assert!(!text.contains("16:34:56"));
}

#[test]
fn json_logs_preserve_name_uid_and_precise_delay_without_color() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    SymptomFixture::write(&path, Some("Mechanical Keyboard"));
    let original = fs::read(&path).unwrap();
    let output = SymptomFixture::view(&path, &["--json"], true);
    assert!(output.status.success());
    assert_eq!(output.stdout, original);
    let record: LogRecord = serde_json::from_slice(&output.stdout).unwrap();
    let LogRecord::Symptom(record) = record else {
        panic!("expected a symptom record");
    };
    assert_eq!(record.press.source.id, "system-keyboard-uid");
    assert_eq!(
        record.press.source.name.as_deref(),
        Some("Mechanical Keyboard")
    );
    assert_eq!(record.interval_ms, 30.75);
    assert_eq!(
        record
            .press
            .timestamp
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        "2026-10-30T16:34:56.987654321Z"
    );
    assert_eq!(record.press.schema_version, 1);
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
