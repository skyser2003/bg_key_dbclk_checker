use crate::{
    detector::Symptom,
    event::{Key, KeyEvent, Source},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PressRecord {
    pub schema_version: u32,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub key: Key,
    pub source: Source,
    pub platform: String,
}

impl PressRecord {
    pub fn new(event: &KeyEvent, session_id: &str) -> Self {
        Self {
            schema_version: 1,
            session_id: session_id.into(),
            timestamp: event.timestamp,
            key: event.key.clone(),
            source: event.source.clone(),
            platform: std::env::consts::OS.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymptomRecord {
    #[serde(flatten)]
    pub press: PressRecord,
    pub previous_timestamp: DateTime<Utc>,
    pub interval_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
}

impl SymptomRecord {
    pub fn new(symptom: &Symptom, session_id: &str) -> Self {
        Self {
            press: PressRecord::new(&symptom.current, session_id),
            previous_timestamp: symptom.previous.timestamp,
            interval_ms: symptom.interval_ms,
            min_ms: symptom.range.min_ms,
            max_ms: symptom.range.max_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
pub enum LogRecord {
    Keystroke(PressRecord),
    Symptom(SymptomRecord),
}

impl LogRecord {
    pub fn press(&self) -> &PressRecord {
        match self {
            Self::Keystroke(p) => p,
            Self::Symptom(s) => &s.press,
        }
    }
    pub fn kind(&self) -> RecordKind {
        match self {
            Self::Keystroke(_) => RecordKind::Keystroke,
            Self::Symptom(_) => RecordKind::Symptom,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum RecordKind {
    All,
    Symptom,
    Keystroke,
}
