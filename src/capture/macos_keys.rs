//! Pure normalization of Quartz keyboard events, also tested on other hosts.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    Press,
    Release,
    Repeat,
}

pub(crate) fn is_software_posted(source_pid: i64, source_state: i64) -> bool {
    // Hardware has no originating Unix process. A nonzero source PID marks
    // process-posted events even when an injector chooses the HID state. A
    // private state belongs to a software-created source. Combined-session
    // state alone does not prove injection and must not reject hardware.
    source_pid > 0 || source_state == -1
}

pub(crate) fn action(
    event_type: u32,
    key_code: u16,
    flags: u64,
    autorepeat: bool,
) -> Option<Action> {
    match event_type {
        10 => Some(if autorepeat {
            Action::Repeat
        } else {
            Action::Press
        }),
        11 => Some(Action::Release),
        12 => {
            // IOKit IOLLEvent.h device-dependent masks distinguish both sides
            // of a modifier. The general Shift flag cannot distinguish a
            // released Left Shift while Right Shift remains down. Caps Lock
            // uses its stateless physical flag, never its latched lock state.
            let mask = match key_code {
                54 => 0x10,
                55 => 0x8,
                56 => 0x2,
                57 => 0x1000000 | 0x80,
                58 => 0x20,
                59 => 0x1,
                60 => 0x4,
                61 => 0x40,
                62 => 0x2000,
                63 => 0x800000,
                _ => return None,
            };
            Some(if flags & mask != 0 {
                Action::Press
            } else {
                Action::Release
            })
        }
        _ => None,
    }
}

pub(crate) fn key_name(code: u16) -> String {
    // These are physical ANSI positions / native virtual key names from
    // HIToolbox Events.h, independent of the active keyboard layout.
    let name = match code {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        10 => "ISOSection",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        22 => "Digit6",
        23 => "Digit5",
        24 => "Equal",
        25 => "Digit9",
        26 => "Digit7",
        27 => "Minus",
        28 => "Digit8",
        29 => "Digit0",
        30 => "BracketRight",
        31 => "KeyO",
        32 => "KeyU",
        33 => "BracketLeft",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Enter",
        37 => "KeyL",
        38 => "KeyJ",
        39 => "Quote",
        40 => "KeyK",
        41 => "Semicolon",
        42 => "Backslash",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "Backquote",
        51 => "Backspace",
        53 => "Escape",
        54 => "RightCommand",
        55 => "LeftCommand",
        56 => "LeftShift",
        57 => "CapsLock",
        58 => "LeftOption",
        59 => "LeftControl",
        60 => "RightShift",
        61 => "RightOption",
        62 => "RightControl",
        63 => "Fn",
        64 => "F17",
        65 => "NumpadDecimal",
        67 => "NumpadMultiply",
        69 => "NumpadAdd",
        71 => "NumpadClear",
        72 => "VolumeUp",
        73 => "VolumeDown",
        74 => "Mute",
        75 => "NumpadDivide",
        76 => "NumpadEnter",
        78 => "NumpadSubtract",
        79 => "F18",
        80 => "F19",
        81 => "NumpadEqual",
        82 => "Numpad0",
        83 => "Numpad1",
        84 => "Numpad2",
        85 => "Numpad3",
        86 => "Numpad4",
        87 => "Numpad5",
        88 => "Numpad6",
        89 => "Numpad7",
        90 => "F20",
        91 => "Numpad8",
        92 => "Numpad9",
        93 => "JISYen",
        94 => "JISUnderscore",
        95 => "JISNumpadComma",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        102 => "JISEisu",
        103 => "F11",
        104 => "JISKana",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        111 => "F12",
        113 => "F15",
        114 => "Help",
        115 => "Home",
        116 => "PageUp",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "PageDown",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return format!("KeyCode{code}"),
    };
    name.to_owned()
}

pub(crate) fn ticks_to_nanoseconds(ticks: u64, numer: u32, denom: u32) -> Option<u64> {
    let ns = (u128::from(ticks) * u128::from(numer)).checked_div(u128::from(denom))?;
    u64::try_from(ns).ok()
}

pub(crate) fn timestamp_offset_ns(event_ns: u64, now_ns: u64) -> Option<i64> {
    i64::try_from(i128::from(event_ns) - i128::from(now_ns)).ok()
}

pub(crate) fn auxiliary_key(subtype: i16, data1: i64) -> Option<(u32, String, Action)> {
    // NX_SUBTYPE_AUX_CONTROL_BUTTONS, as used for media/brightness keys.
    if subtype != 8 {
        return None;
    }
    let bits = data1 as u32;
    let key_type = bits >> 16;
    let action = match (bits >> 8) & 0xff {
        0x0a if bits & 1 != 0 => Action::Repeat,
        0x0a => Action::Press,
        0x0b => Action::Release,
        _ => return None,
    };
    // IOKit ev_keymap.h NX_KEYTYPE_* values live in a separate namespace from
    // ordinary virtual key codes. Retain even an unrecognized auxiliary key.
    let name = match key_type {
        0 => "VolumeUp",
        1 => "VolumeDown",
        2 => "BrightnessUp",
        3 => "BrightnessDown",
        4 => "CapsLock",
        5 => "Help",
        6 => "Power",
        7 => "Mute",
        10 => "NumLock",
        11 => "ContrastUp",
        12 => "ContrastDown",
        13 => "LaunchPanel",
        14 => "Eject",
        15 => "VideoMirror",
        16 => "MediaPlayPause",
        17 => "MediaNext",
        18 => "MediaPrevious",
        19 => "MediaFastForward",
        20 => "MediaRewind",
        21 => "KeyboardBrightnessUp",
        22 => "KeyboardBrightnessDown",
        23 => "KeyboardBacklightToggle",
        25 => "Menu",
        _ => {
            return Some((
                0x10000 | key_type,
                format!("AuxiliaryKey{key_type}"),
                action,
            ))
        }
    };
    Some((0x10000 | key_type, name.to_owned(), action))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_key_events_preserve_explicit_repeats() {
        assert_eq!(action(10, 0, 0, false), Some(Action::Press));
        assert_eq!(action(10, 0, 0, true), Some(Action::Repeat));
        assert_eq!(action(11, 0, 0, false), Some(Action::Release));
        assert_eq!(action(14, 0, 0, false), None);
    }

    #[test]
    fn modifier_release_is_independent_of_the_opposite_modifier() {
        // Right Shift remains held while Left Shift is released.
        assert_eq!(action(12, 56, 0x20000 | 0x4, false), Some(Action::Release));
        assert_eq!(action(12, 60, 0x20000 | 0x4, false), Some(Action::Press));
        assert_eq!(
            action(12, 59, 0x40000 | 0x2000, false),
            Some(Action::Release)
        );
        assert_eq!(action(12, 62, 0x40000 | 0x2000, false), Some(Action::Press));
    }

    #[test]
    fn all_modifier_keys_use_physical_down_flags() {
        for (code, flag) in [
            (54, 0x10),
            (55, 0x8),
            (56, 0x2),
            (58, 0x20),
            (59, 0x1),
            (60, 0x4),
            (61, 0x40),
            (62, 0x2000),
            (63, 0x800000),
        ] {
            assert_eq!(action(12, code, flag, false), Some(Action::Press));
            assert_eq!(action(12, code, 0, false), Some(Action::Release));
        }
        assert_eq!(action(12, 0, 0, false), None);
    }

    #[test]
    fn caps_lock_uses_stateless_physical_state_not_latched_lock_state() {
        assert_eq!(
            action(12, 57, 0x1000000 | 0x10000, false),
            Some(Action::Press)
        );
        assert_eq!(action(12, 57, 0x10000, false), Some(Action::Release));
        assert_eq!(action(12, 57, 0x80, false), Some(Action::Press));
        assert_eq!(action(12, 57, 0, false), Some(Action::Release));
    }

    #[test]
    fn software_metadata_excludes_process_posted_and_private_source_events() {
        // Do not require HID source state 1: state 0 can describe hardware
        // in the combined session stream, depending on tap location.
        assert!(!is_software_posted(0, 1));
        assert!(!is_software_posted(0, 0));
        assert!(is_software_posted(1234, 0));
        assert!(is_software_posted(1234, 1));
        assert!(is_software_posted(0, -1));
    }

    #[test]
    fn native_codes_have_stable_physical_names_including_modifiers() {
        assert_eq!(key_name(0), "KeyA");
        assert_eq!(key_name(54), "RightCommand");
        assert_eq!(key_name(56), "LeftShift");
        assert_eq!(key_name(57), "CapsLock");
        assert_eq!(key_name(123), "ArrowLeft");
        assert_eq!(key_name(255), "KeyCode255");
    }

    #[test]
    fn mach_time_conversion_handles_fractional_timebase_and_overflow() {
        assert_eq!(ticks_to_nanoseconds(9, 125, 3), Some(375));
        assert_eq!(ticks_to_nanoseconds(u64::MAX, 1, 1), Some(u64::MAX));
        assert_eq!(ticks_to_nanoseconds(u64::MAX, 2, 1), None);
        assert_eq!(ticks_to_nanoseconds(123, 1, 0), None);
    }

    #[test]
    fn native_event_time_preserves_delay_before_callback() {
        assert_eq!(
            timestamp_offset_ns(1_000_000_000, 1_050_000_000),
            Some(-50_000_000)
        );
        assert_eq!(
            timestamp_offset_ns(1_050_000_000, 1_000_000_000),
            Some(50_000_000)
        );
        assert_eq!(timestamp_offset_ns(0, u64::MAX), None);
    }

    #[test]
    fn auxiliary_volume_and_play_keys_preserve_press_release_and_repeat() {
        assert_eq!(
            auxiliary_key(8, 0x0a00),
            Some((0x10000, "VolumeUp".into(), Action::Press))
        );
        assert_eq!(
            auxiliary_key(8, 0x0b00),
            Some((0x10000, "VolumeUp".into(), Action::Release))
        );
        assert_eq!(
            auxiliary_key(8, 0x0a01),
            Some((0x10000, "VolumeUp".into(), Action::Repeat))
        );
        assert_eq!(
            auxiliary_key(8, (16 << 16) | 0x0a00),
            Some((0x10010, "MediaPlayPause".into(), Action::Press))
        );
    }

    #[test]
    fn auxiliary_keys_skip_non_keyboard_system_events_and_keep_unknown_codes() {
        assert_eq!(auxiliary_key(1, 0x0a00), None);
        assert_eq!(auxiliary_key(8, 0x0100), None);
        assert_eq!(
            auxiliary_key(8, (40 << 16) | 0x0a00),
            Some((0x10028, "AuxiliaryKey40".into(), Action::Press))
        );
    }
}
