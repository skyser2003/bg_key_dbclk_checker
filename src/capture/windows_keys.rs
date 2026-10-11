//! Windows keyboard normalization without native API dependencies.

use std::collections::{HashMap, HashSet};

use crate::event::KeyAction;

pub(super) const PAUSE_CODE: u32 = 0x245;

#[derive(Default)]
pub(super) struct PacketNormalizer {
    pending_e1: bool,
}
impl PacketNormalizer {
    pub(super) fn normalize(
        &mut self,
        make_code: u16,
        flags: u16,
        virtual_key: u16,
    ) -> Option<u32> {
        if flags & 4 != 0 {
            if make_code == 0x45 {
                self.pending_e1 = false;
                return Some(PAUSE_CODE);
            }
            self.pending_e1 = true;
            return None;
        }
        if flags & 2 != 0 && make_code == 0x2a {
            return None;
        }
        if std::mem::take(&mut self.pending_e1) {
            if make_code == 0x45 {
                return Some(PAUSE_CODE);
            }
            return physical_code(make_code, flags | 4, virtual_key);
        }
        physical_code(make_code, flags, virtual_key)
    }
}

pub(super) fn physical_code(make_code: u16, flags: u16, virtual_key: u16) -> Option<u32> {
    if make_code == 0xff || virtual_key == 0xff || (make_code == 0 && virtual_key == 0) {
        return None;
    }
    if virtual_key == 0x13 {
        return Some(PAUSE_CODE);
    }
    if make_code == 0 {
        // Some media keys have no scan code. Keep their fallback identity in a
        // separate namespace so it cannot collide with a physical scan code.
        return Some(0x1_0000 | u32::from(virtual_key));
    }
    Some(u32::from(make_code) | (u32::from(flags & 6) << 7))
}

pub(super) fn canonical_virtual_key(make_code: u16, flags: u16, virtual_key: u16) -> u16 {
    match virtual_key {
        0x10 if make_code == 0x36 => 0xa1,
        0x10 => 0xa0,
        0x11 if flags & 2 != 0 => 0xa3,
        0x11 => 0xa2,
        0x12 if flags & 2 != 0 => 0xa5,
        0x12 => 0xa4,
        _ => virtual_key,
    }
}

pub(super) fn message_ticks(message_time: u32, current_ticks: u64) -> u64 {
    // MSG.time contains only the low 32 bits of the boot clock. Recover the
    // most recent matching instant, including a message queued before wrap.
    current_ticks.saturating_sub(u64::from((current_ticks as u32).wrapping_sub(message_time)))
}

pub(super) fn event_timestamp(
    event_ticks: u64,
    current_ticks: u64,
    current_wall: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let age = i64::try_from(current_ticks.checked_sub(event_ticks)?).ok()?;
    current_wall.checked_sub_signed(chrono::TimeDelta::milliseconds(age))
}

pub(super) struct KeyState {
    startup_held: HashSet<u16>,
    startup_physical: HashMap<u32, HashSet<u16>>,
    held: HashSet<u32>,
}

impl KeyState {
    pub(super) fn new(startup_held: HashSet<u16>) -> Self {
        Self {
            startup_held,
            startup_physical: HashMap::new(),
            held: HashSet::new(),
        }
    }

    pub(super) fn guard_startup_physical_key(&mut self, code: u32, virtual_key: u16) {
        self.startup_physical
            .entry(code)
            .or_default()
            .insert(virtual_key);
    }

    pub(super) fn classify(&mut self, code: u32, virtual_key: u16, flags: u16) -> KeyAction {
        if flags & 1 != 0 {
            if let Some(original_keys) = self.startup_physical.remove(&code) {
                for original in original_keys {
                    self.startup_held.remove(&original);
                }
            }
            self.startup_held.remove(&virtual_key);
            self.held.remove(&code);
            return KeyAction::Release;
        }
        if self.startup_physical.contains_key(&code)
            || self.startup_held.contains(&virtual_key)
            || !self.held.insert(code)
        {
            return KeyAction::Repeat;
        }
        KeyAction::Press
    }

    pub(super) fn classify_at_time(
        &mut self,
        code: u32,
        virtual_key: u16,
        flags: u16,
        event_ticks: u64,
        cutoff: u64,
    ) -> KeyAction {
        let action = self.classify(code, virtual_key, flags);
        // Replay older transitions into held-state only. In particular an old
        // raw repeat whose release preceded the snapshot is not a physical press.
        if event_ticks <= cutoff && action == KeyAction::Press {
            KeyAction::Repeat
        } else {
            action
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_code_identity_distinguishes_extension_prefixes() {
        assert_eq!(physical_code(0x1c, 0, 0x0d), Some(0x1c));
        assert_eq!(physical_code(0x1c, 2, 0x0d), Some(0x11c));
        assert_eq!(physical_code(0x1d, 4, 0x13), Some(PAUSE_CODE));
        assert_eq!(physical_code(0x1c, 1, 0x0d), Some(0x1c));
    }

    #[test]
    fn pause_prefix_sequence_is_one_key_and_not_num_lock() {
        let mut packets = PacketNormalizer::default();
        assert_eq!(packets.normalize(0x1d, 4, 0x11), None);
        assert_eq!(packets.normalize(0x45, 0, 0x90), Some(PAUSE_CODE));
        assert_eq!(packets.normalize(0x1d, 5, 0x11), None);
        assert_eq!(packets.normalize(0x45, 1, 0x90), Some(PAUSE_CODE));
        assert_ne!(packets.normalize(0x45, 0, 0x90), Some(PAUSE_CODE));
        assert_eq!(packets.normalize(0x45, 4, 0x13), Some(PAUSE_CODE));
    }

    #[test]
    fn fake_extended_shift_is_ignored_and_prefix_state_is_per_keyboard() {
        let mut one = PacketNormalizer::default();
        let mut two = PacketNormalizer::default();
        assert_eq!(one.normalize(0x2a, 2, 0x10), None);
        assert_eq!(one.normalize(0x1d, 4, 0xff), None);
        assert_eq!(two.normalize(0x45, 0, 0x90), Some(0x45));
        assert_eq!(one.normalize(0x45, 0, 0x90), Some(PAUSE_CODE));
        assert_eq!(one.normalize(0x37, 2, 0x2c), Some(0x137));
    }

    #[test]
    fn ignores_overruns_and_preserves_media_keys_without_scan_codes() {
        assert_eq!(physical_code(0xff, 0, 0x41), None);
        assert_eq!(physical_code(0x1e, 0, 0xff), None);
        assert_eq!(physical_code(0, 0, 0), None);
        assert_eq!(physical_code(0, 0, 0xb3), Some(0x1_00b3));
        assert_ne!(physical_code(0, 0, 0xb3), physical_code(0xb3, 0, 0x41));
    }

    #[test]
    fn physical_identity_is_independent_of_virtual_key_layout() {
        assert_eq!(physical_code(0x10, 0, 0x51), physical_code(0x10, 0, 0x41));
    }

    #[test]
    fn resolves_modifier_sides_for_startup_state() {
        assert_eq!(canonical_virtual_key(0x2a, 0, 0x10), 0xa0);
        assert_eq!(canonical_virtual_key(0x36, 0, 0x10), 0xa1);
        assert_eq!(canonical_virtual_key(0x1d, 0, 0x11), 0xa2);
        assert_eq!(canonical_virtual_key(0x1d, 2, 0x11), 0xa3);
        assert_eq!(canonical_virtual_key(0x38, 0, 0x12), 0xa4);
        assert_eq!(canonical_virtual_key(0x38, 2, 0x12), 0xa5);
        assert_eq!(canonical_virtual_key(0x1e, 0, 0x41), 0x41);
    }

    #[test]
    fn message_clock_retains_native_event_time_despite_queue_delay() {
        assert_eq!(message_ticks(100, 500), 100);
        assert_eq!(message_ticks(0xffff_fffa, 0x1_0000_0005), 0xffff_fffa);
        assert_eq!(message_ticks(5, 0x1_0000_0007), 0x1_0000_0005);
    }

    #[test]
    fn native_event_utc_uses_current_wall_clock_and_subtracts_queue_delay() {
        use chrono::{TimeDelta, TimeZone, Utc};
        let initial_wall = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let adjusted_wall = initial_wall - TimeDelta::hours(2);
        assert_eq!(
            event_timestamp(1_950, 2_000, adjusted_wall),
            Some(adjusted_wall - TimeDelta::milliseconds(50))
        );
        assert_eq!(
            event_timestamp(1_950, 2_000, initial_wall),
            Some(initial_wall - TimeDelta::milliseconds(50))
        );
    }

    #[test]
    fn held_key_downs_repeat_until_a_release() {
        let mut state = KeyState::new(HashSet::new());
        assert_eq!(state.classify(0x1e, 0x41, 0), KeyAction::Press);
        assert_eq!(state.classify(0x1e, 0x41, 0), KeyAction::Repeat);
        assert_eq!(state.classify(0x1e, 0x41, 1), KeyAction::Release);
        assert_eq!(state.classify(0x1e, 0x41, 0), KeyAction::Press);
    }

    #[test]
    fn startup_held_keys_are_excluded_until_released() {
        let mut state = KeyState::new(HashSet::from([0x41]));
        assert_eq!(state.classify(0x1e, 0x41, 0), KeyAction::Repeat);
        assert_eq!(state.classify(0x1e, 0x41, 0), KeyAction::Repeat);
        assert_eq!(state.classify(0x1e, 0x41, 1), KeyAction::Release);
        assert_eq!(state.classify(0x1e, 0x41, 0), KeyAction::Press);
    }

    #[test]
    fn startup_guard_tracks_physical_key_when_layout_changes() {
        let mut state = KeyState::new(HashSet::from([0x51]));
        state.guard_startup_physical_key(0x10, 0x51);
        // The held physical Q-position now maps to virtual A after a layout
        // change. Neither its repeat nor its release is a new physical press.
        assert_eq!(state.classify(0x10, 0x41, 0), KeyAction::Repeat);
        assert_eq!(state.classify(0x10, 0x41, 1), KeyAction::Release);
        assert_eq!(state.classify(0x10, 0x41, 0), KeyAction::Press);
        // The startup virtual Q guard must also have been retired, because it
        // can now refer to a different physical position under the new layout.
        assert_eq!(state.classify(0x1e, 0x51, 0), KeyAction::Press);
    }

    #[test]
    fn modifier_startup_state_does_not_clear_other_side_on_release() {
        let mut state = KeyState::new(HashSet::from([0xa0, 0xa1]));
        assert_eq!(state.classify(0x2a, 0xa0, 1), KeyAction::Release);
        assert_eq!(state.classify(0x36, 0xa1, 0), KeyAction::Repeat);
        assert_eq!(state.classify(0x2a, 0xa0, 0), KeyAction::Press);
    }

    #[test]
    fn keyboard_state_is_independent_between_devices() {
        let mut first = KeyState::new(HashSet::new());
        let mut second = KeyState::new(HashSet::new());
        assert_eq!(first.classify(0x1e, 0x41, 0), KeyAction::Press);
        assert_eq!(second.classify(0x1e, 0x41, 0), KeyAction::Press);
        assert_eq!(first.classify(0x1e, 0x41, 0), KeyAction::Repeat);
    }

    #[test]
    fn queued_events_before_startup_snapshot_only_seed_state() {
        let mut state = KeyState::new(HashSet::new());
        // Snapshot sees the key already released, but a pre-snapshot repeat
        // followed by its release is still queued in the registered stream.
        assert_eq!(
            state.classify_at_time(0x1e, 0x41, 0, 90, 100),
            KeyAction::Repeat
        );
        assert_eq!(
            state.classify_at_time(0x1e, 0x41, 1, 95, 100),
            KeyAction::Release
        );
        assert_eq!(
            state.classify_at_time(0x1e, 0x41, 0, 110, 100),
            KeyAction::Press
        );
        let mut held = KeyState::new(HashSet::new());
        // A down during snapshot collection must still guard later repeats.
        assert_eq!(
            held.classify_at_time(0x1e, 0x41, 0, 100, 100),
            KeyAction::Repeat
        );
        assert_eq!(
            held.classify_at_time(0x1e, 0x41, 0, 120, 100),
            KeyAction::Repeat
        );
    }
}
