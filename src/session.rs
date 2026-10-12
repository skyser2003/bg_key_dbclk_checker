use crate::{
    detector::{Detector, Range},
    event::{CaptureMessage, Key, Source},
    record::{LogRecord, PressRecord, SymptomRecord},
    storage::LogStore,
};
use std::{
    collections::HashMap,
    io::{self, Write},
};

struct SymptomCount {
    key: Key,
    source: Source,
    count: u64,
}

pub struct Session {
    detector: Detector,
    all_keys: bool,
    store: LogStore,
    session_id: String,
    symptom_counts: HashMap<(String, u32), SymptomCount>,
}
impl Session {
    pub fn new(range: Range, all_keys: bool, store: LogStore, session_id: String) -> Self {
        Self {
            detector: Detector::new(range),
            all_keys,
            store,
            session_id,
            symptom_counts: HashMap::new(),
        }
    }
    pub fn handle(&mut self, message: CaptureMessage) -> anyhow::Result<Option<SymptomRecord>> {
        let event = match message {
            CaptureMessage::Reset { source, held_keys } => {
                self.detector.reset_source(&source, held_keys);
                return Ok(None);
            }
            CaptureMessage::Event(event) => event,
        };
        let result = self.detector.process(event);
        if self.all_keys {
            if let Some(press) = result.accepted_press {
                self.store.append(&LogRecord::Keystroke(PressRecord::new(
                    &press,
                    &self.session_id,
                )))?;
            }
        }
        if let Some(symptom) = result.symptom {
            let record = SymptomRecord::new(&symptom, &self.session_id);
            self.store.append(&LogRecord::Symptom(record.clone()))?;
            let count = self
                .symptom_counts
                .entry((record.press.source.id.clone(), record.press.key.code))
                .or_insert_with(|| SymptomCount {
                    key: record.press.key.clone(),
                    source: record.press.source.clone(),
                    count: 0,
                });
            count.count += 1;
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    /// Report successfully saved symptoms from this session, grouped by source/key.
    pub fn write_summary(&self, output: &mut impl Write) -> io::Result<()> {
        let mut counts: Vec<_> = self.symptom_counts.values().collect();
        counts.sort_unstable_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.key.name.cmp(&b.key.name))
                .then_with(|| a.key.code.cmp(&b.key.code))
                .then_with(|| a.source.id.cmp(&b.source.id))
        });
        let total: u64 = counts.iter().map(|entry| entry.count).sum();
        writeln!(output, "\nDouble-click summary (current session)")?;
        writeln!(output, "Total symptoms: {total}")?;
        if counts.is_empty() {
            writeln!(output, "No double-click symptoms detected.")?;
        }
        for entry in counts {
            writeln!(
                output,
                "  {} key={:?} code={} source={:?}",
                entry.count, entry.key.name, entry.key.code, entry.source.id
            )?;
        }
        Ok(())
    }
}
