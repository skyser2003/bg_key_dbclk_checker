use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: Option<String>,
}

impl Source {
    pub(crate) fn display_name(&self) -> String {
        let name = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if self.id.starts_with("rawinput:") {
            // A deterministic FNV-1a tag distinguishes identical model names
            // without exposing the full device path in every terminal line.
            let tag = self.id.bytes().fold(0x811c9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
            });
            format!("{} [{tag:08x}]", name.unwrap_or("Keyboard"))
        } else {
            name.unwrap_or(&self.id).to_owned()
        }
    }
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

#[cfg(test)]
mod tests;
