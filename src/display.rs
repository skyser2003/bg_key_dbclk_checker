use crate::{
    detector::Range,
    event::{Key, Source},
    record::LogRecord,
    storage::SavedRecord,
};
use chrono::{DateTime, Local, Utc};
use clap::builder::styling::{AnsiColor, Style};
use comfy_table::{presets::ASCII_MARKDOWN, CellAlignment, ContentArrangement, Table};
use std::{
    io::{self, Write},
    path::Path,
};

mod monitor;
pub(crate) use monitor::MonitorDisplay;

pub(crate) struct TerminalFormat {
    timestamp_style: Style,
    key_style: Style,
    interval_style: Style,
}

impl TerminalFormat {
    pub(crate) fn new(color: bool) -> Self {
        Self {
            timestamp_style: if color {
                AnsiColor::Green.on_default()
            } else {
                Style::new()
            },
            key_style: if color {
                AnsiColor::Cyan.on_default()
            } else {
                Style::new()
            },
            interval_style: if color {
                AnsiColor::Yellow.on_default()
            } else {
                Style::new()
            },
        }
    }

    pub(crate) fn key(&self, name: &str) -> String {
        let style = self.key_style;
        // Escape device-supplied control characters before applying our styles.
        format!("{style}{name:?}{style:#}")
    }

    pub(crate) fn interval(&self, interval_ms: f64) -> String {
        let style = self.interval_style;
        format!("{style}{interval_ms:.0} ms{style:#}")
    }

    pub(crate) fn timestamp(&self, timestamp: &DateTime<Utc>) -> String {
        let style = self.timestamp_style;
        let timestamp = timestamp.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S");
        format!("{style}{timestamp}{style:#}")
    }

    pub(crate) fn write_logs(
        &self,
        records: &[SavedRecord],
        output: &mut impl Write,
    ) -> io::Result<()> {
        let mut table = Self::table(&["Time (local)", "Type", "Key", "Code", "Delay", "Keyboard"]);
        for saved in records {
            let press = saved.record.press();
            let delay = match &saved.record {
                LogRecord::Symptom(symptom) => self.interval(symptom.interval_ms),
                LogRecord::Keystroke(_) => "—".into(),
            };
            table.add_row([
                self.timestamp(&press.timestamp),
                format!("{:?}", saved.record.kind()),
                self.key(&press.key.name),
                press.key.code.to_string(),
                delay,
                format!("{:?}", press.source.display_name()),
            ]);
        }
        writeln!(output, "{table}")
    }

    pub(crate) fn write_monitor_status(
        &self,
        output: &mut impl Write,
        path: &Path,
        range: Range,
        all_keys: bool,
    ) -> io::Result<()> {
        writeln!(output, "Keyboard chatter monitor\n")?;
        writeln!(
            output,
            "  Detection  {}–{} ms (inclusive)",
            range.min_ms, range.max_ms
        )?;
        writeln!(
            output,
            "  Recording  {}",
            if all_keys {
                "Symptoms and all distinct physical key presses"
            } else {
                "Symptoms"
            }
        )?;
        writeln!(output, "  Log file   {}", path.display())?;
        writeln!(
            output,
            "\nPress Ctrl+C to stop and show the session summary."
        )?;
        output.flush()
    }

    pub(crate) fn write_counts<'a>(
        &self,
        output: &mut impl Write,
        counts: impl Iterator<Item = (u64, &'a Key, &'a Source)>,
    ) -> io::Result<()> {
        let mut table = Self::table(&["Count", "Key", "Code", "Keyboard"]);
        for (count, key, source) in counts {
            table.add_row([
                count.to_string(),
                self.key(&key.name),
                key.code.to_string(),
                format!("{:?}", source.display_name()),
            ]);
        }
        for index in [0, 2] {
            if let Some(column) = table.column_mut(index) {
                column.set_cell_alignment(CellAlignment::Right);
            }
        }
        writeln!(output, "{table}")
    }

    fn table(headers: &[&str]) -> Table {
        let mut table = Table::new();
        table
            .load_preset(ASCII_MARKDOWN)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(headers.iter().copied());
        if !table.is_tty() {
            table.set_width(120);
        }
        table
    }
}
