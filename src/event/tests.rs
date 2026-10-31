use super::Source;

#[test]
fn identical_windows_keyboard_names_get_distinct_stable_tags() {
    let first = Source {
        id: "rawinput:keyboard-a".into(),
        name: Some("HID Keyboard Device".into()),
    };
    let second = Source {
        id: "rawinput:keyboard-b".into(),
        name: first.name.clone(),
    };
    let first_label = first.display_name();
    assert!(first_label.starts_with("HID Keyboard Device ["));
    assert!(first_label.ends_with(']'));
    assert_eq!(first_label.len(), "HID Keyboard Device []".len() + 8);
    assert_ne!(first_label, second.display_name());
    assert_eq!(first_label, first.clone().display_name());
    assert!(!first_label.contains("rawinput:"));
    assert_eq!(first.name.as_deref(), Some("HID Keyboard Device"));
    assert_eq!(first.id, "rawinput:keyboard-a");
}

#[test]
fn unnamed_windows_keyboard_uses_a_readable_label_and_tag() {
    let source = Source {
        id: "rawinput:handle:1234".into(),
        name: None,
    };
    assert!(source.display_name().starts_with("Keyboard ["));
    assert!(!source.display_name().contains("rawinput:"));
}
