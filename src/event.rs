use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub code: u32,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAction {
    Press,
    Release,
    Repeat,
}

#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub source: Source,
    pub key: Key,
    pub action: KeyAction,
    pub monotonic_ns: u64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub enum CaptureMessage {
    Event(KeyEvent),
    Reset { source: Source, held_keys: Vec<Key> },
}
