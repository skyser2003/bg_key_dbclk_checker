use crate::{
    detector::{Detector, Range},
    event::CaptureMessage,
    record::{LogRecord, PressRecord, SymptomRecord},
    storage::LogStore,
};
pub struct Session {
    detector: Detector,
    all_keys: bool,
    store: LogStore,
    session_id: String,
}
impl Session {
    pub fn new(range: Range, all_keys: bool, store: LogStore, session_id: String) -> Self {
        Self {
            detector: Detector::new(range),
            all_keys,
            store,
            session_id,
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
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }
}
