use crate::event::{Key, KeyAction, KeyEvent, Source};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub min_ms: f64,
    pub max_ms: f64,
}

impl Range {
    pub fn new(min_ms: f64, max_ms: f64) -> anyhow::Result<Self> {
        anyhow::ensure!(
            min_ms.is_finite()
                && max_ms.is_finite()
                && min_ms >= 0.0
                && max_ms > 0.0
                && min_ms <= max_ms,
            "range must have finite bounds with 0 <= min-ms <= max-ms and max-ms > 0"
        );
        Ok(Self { min_ms, max_ms })
    }
}

#[derive(Clone, Debug)]
pub struct Symptom {
    pub previous: KeyEvent,
    pub current: KeyEvent,
    pub interval_ms: f64,
    pub range: Range,
}

#[derive(Default, Debug)]
pub struct Detection {
    pub accepted_press: Option<KeyEvent>,
    pub symptom: Option<Symptom>,
}

#[derive(Default)]
struct KeyState {
    held: bool,
    previous: Option<KeyEvent>,
}

pub struct Detector {
    range: Range,
    states: HashMap<(String, u32), KeyState>,
}

impl Detector {
    pub fn new(range: Range) -> Self {
        Self {
            range,
            states: HashMap::new(),
        }
    }

    pub fn process(&mut self, event: KeyEvent) -> Detection {
        let state = self
            .states
            .entry((event.source.id.clone(), event.key.code))
            .or_default();
        match event.action {
            KeyAction::Release => {
                state.held = false;
                return Detection::default();
            }
            KeyAction::Repeat => {
                // A repeat also proves the key is held when capture starts mid-hold.
                state.held = true;
                return Detection::default();
            }
            KeyAction::Press if state.held => return Detection::default(),
            KeyAction::Press => state.held = true,
        }
        let symptom = state.previous.as_ref().and_then(|previous| {
            let ns = event.monotonic_ns.checked_sub(previous.monotonic_ns)?;
            let interval_ms = ns as f64 / 1_000_000.0;
            (interval_ms >= self.range.min_ms && interval_ms <= self.range.max_ms).then(|| {
                Symptom {
                    previous: previous.clone(),
                    current: event.clone(),
                    interval_ms,
                    range: self.range,
                }
            })
        });
        state.previous = Some(event.clone());
        Detection {
            accepted_press: Some(event),
            symptom,
        }
    }

    pub fn reset_source(&mut self, source: &Source, held: Vec<Key>) {
        self.states.retain(|(id, _), _| id != &source.id);
        for key in held {
            self.states.insert(
                (source.id.clone(), key.code),
                KeyState {
                    held: true,
                    previous: None,
                },
            );
        }
    }
}
