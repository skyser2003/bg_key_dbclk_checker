use super::TerminalFormat;
use crate::record::SymptomRecord;
use comfy_table::{CellAlignment, ColumnConstraint, Table, Width};
use std::io::{self, Write};

pub(crate) struct MonitorDisplay<'a> {
    format: &'a TerminalFormat,
    width: u16,
    column_widths: [u16; 5],
}

impl<'a> MonitorDisplay<'a> {
    pub(crate) fn for_terminal(format: &'a TerminalFormat) -> Self {
        Self::new(format, Table::new().width().unwrap_or(120))
    }

    fn new(format: &'a TerminalFormat, width: u16) -> Self {
        let width = width.clamp(40, 120);
        // Constraints include cell padding; six separators surround five cells.
        let available = width - 6;
        let time = (available / 3).min(21);
        let key = (available / 5).clamp(5, 14);
        let code = (available / 8).clamp(4, 7);
        let delay = (available / 6).clamp(5, 9);
        let keyboard = available - time - key - code - delay;
        Self {
            format,
            width,
            column_widths: [time, key, code, delay, keyboard],
        }
    }

    pub(crate) fn write_header(&self, output: &mut impl Write) -> io::Result<()> {
        writeln!(output, "\nSaved double-click symptoms")?;
        writeln!(output, "{}", self.table())?;
        output.flush()
    }

    pub(crate) fn write_symptom(
        &self,
        output: &mut impl Write,
        symptom: &SymptomRecord,
    ) -> io::Result<()> {
        // Render only this row: an indefinitely running monitor never needs to
        // retain or redraw its history. Fixed widths keep later rows aligned.
        let mut table = self.table();
        let header_lines = table.lines().count();
        let press = &symptom.press;
        table.add_row([
            self.format.timestamp(&press.timestamp),
            self.format.key(&press.key.name),
            press.key.code.to_string(),
            self.format.interval(symptom.interval_ms),
            format!("{:?}", press.source.display_name()),
        ]);
        for line in table.lines().skip(header_lines) {
            writeln!(output, "{line}")?;
        }
        output.flush()
    }

    fn table(&self) -> Table {
        let mut table =
            TerminalFormat::table(&["Time (local)", "Key", "Code", "Delay", "Keyboard"]);
        table.set_width(self.width).set_constraints(
            self.column_widths
                .map(|width| ColumnConstraint::Absolute(Width::Fixed(width))),
        );
        for index in [2, 3] {
            if let Some(column) = table.column_mut(index) {
                column.set_cell_alignment(CellAlignment::Right);
            }
        }
        table
    }
}

#[cfg(test)]
mod tests;
