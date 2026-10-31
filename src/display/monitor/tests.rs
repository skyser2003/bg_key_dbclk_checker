use super::{MonitorDisplay, TerminalFormat};
use crate::{
    event::{Key, KeyAction, KeyEvent, Source},
    record::{PressRecord, SymptomRecord},
};
use std::io::{self, Write};

struct MonitorBuffer {
    bytes: Vec<u8>,
    flushes: usize,
}

impl MonitorBuffer {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            flushes: 0,
        }
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.bytes).unwrap()
    }

    fn symptom(name: &str) -> SymptomRecord {
        let event = KeyEvent {
            source: Source {
                id: "keyboard".into(),
                name: Some(name.into()),
            },
            key: Key {
                code: 30,
                name: "KEY_A".into(),
            },
            action: KeyAction::Press,
            monotonic_ns: 0,
            timestamp: chrono::Utc::now(),
        };
        SymptomRecord {
            press: PressRecord::new(&event, "test"),
            previous_timestamp: event.timestamp,
            interval_ms: 30.75,
            min_ms: 0.0,
            max_ms: 80.0,
        }
    }
}

impl Write for MonitorBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn live_rows_keep_header_alignment_when_names_wrap() {
    let format = TerminalFormat::new(false);
    for width in [40, 60, 80, 120] {
        let display = MonitorDisplay::new(&format, width);
        let mut output = MonitorBuffer::new();
        display.write_header(&mut output).unwrap();
        let first_line = output
            .text()
            .lines()
            .find(|line| line.starts_with('|'))
            .unwrap();
        let positions: Vec<usize> = first_line
            .match_indices('|')
            .map(|(index, _)| index)
            .collect();
        let body_start = output.bytes.len();
        for name in [
            "Short Keyboard",
            "A considerably longer keyboard model whose name must wrap into additional lines",
        ] {
            display
                .write_symptom(&mut output, &MonitorBuffer::symptom(name))
                .unwrap();
        }
        assert_eq!(
            output.text().matches("Saved double-click symptoms").count(),
            1
        );
        for line in output.text().lines().filter(|line| line.starts_with('|')) {
            assert!(line.len() <= usize::from(width), "width={width}: {line}");
            assert_eq!(
                line.match_indices('|')
                    .map(|(index, _)| index)
                    .collect::<Vec<usize>>(),
                positions,
                "width={width}: {line}"
            );
        }
        let keyboard_text: String = output.text()[body_start..]
            .lines()
            .flat_map(|line| line.split('|').nth(5).unwrap().chars())
            .filter(|character| !character.is_whitespace())
            .collect();
        assert_eq!(
            keyboard_text,
            "\"ShortKeyboard\"\"Aconsiderablylongerkeyboardmodelwhosenamemustwrapintoadditionallines\""
        );
    }
}

#[test]
fn header_and_each_saved_row_are_flushed_immediately() {
    let format = TerminalFormat::new(false);
    let display = MonitorDisplay::new(&format, 120);
    let mut output = MonitorBuffer::new();
    display.write_header(&mut output).unwrap();
    assert!(output.text().contains("Time (local)"));
    assert_eq!(output.flushes, 1);
    display
        .write_symptom(&mut output, &MonitorBuffer::symptom("Mechanical Keyboard"))
        .unwrap();
    assert!(output.text().contains("31 ms"));
    assert!(output.text().contains("Mechanical Keyboard"));
    assert_eq!(output.flushes, 2);
}
