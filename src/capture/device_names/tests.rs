use std::collections::BTreeMap;

use super::{DeviceName, DeviceNode, DeviceProperties};

struct DeviceTree {
    nodes: BTreeMap<u32, DeviceNode>,
}

impl DeviceTree {
    fn new(nodes: impl IntoIterator<Item = (u32, DeviceNode)>) -> Self {
        Self {
            nodes: nodes.into_iter().collect(),
        }
    }

    fn keyboard(parent: Option<u32>) -> DeviceNode {
        DeviceNode::new(
            None,
            None,
            Some("HID Keyboard Device".to_owned()),
            Some([1; 16]),
            Some("HID\\VID_1234&PID_5678\\keyboard".to_owned()),
            parent,
        )
    }

    fn product(instance_id: &str, container_id: Option<[u8; 16]>) -> DeviceNode {
        DeviceNode::new(
            Some("Actual Keyboard Model".to_owned()),
            None,
            Some("USB Input Device".to_owned()),
            container_id,
            Some(instance_id.to_owned()),
            None,
        )
    }
}

impl DeviceProperties for DeviceTree {
    fn properties(&self, node: u32) -> DeviceNode {
        self.nodes.get(&node).cloned().unwrap_or_default()
    }
}

#[test]
fn prefers_parent_bus_product_over_generic_hid_description() {
    let tree = DeviceTree::new([
        (1, DeviceTree::keyboard(Some(2))),
        (
            2,
            DeviceTree::product("USB\\VID_1234&PID_5678\\keyboard", Some([1; 16])),
        ),
    ]);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("Actual Keyboard Model")
    );
}

#[test]
fn prefers_local_bus_product_over_friendly_name() {
    let mut keyboard = DeviceTree::keyboard(None);
    keyboard.bus_name = Some("  Local Product  ".to_owned());
    keyboard.friendly_name = Some("Friendly Keyboard".to_owned());
    let tree = DeviceTree::new([(1, keyboard)]);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("Local Product")
    );
}

#[test]
fn refuses_parent_product_outside_known_nonzero_container() {
    for container in [None, Some([0; 16]), Some([2; 16])] {
        let tree = DeviceTree::new([
            (1, DeviceTree::keyboard(Some(2))),
            (
                2,
                DeviceTree::product("USB\\VID_1234&PID_5678\\keyboard", container),
            ),
        ]);

        assert_eq!(
            DeviceName::lookup(&tree, 1).as_deref(),
            Some("HID Keyboard Device")
        );
    }
}

#[test]
fn missing_or_zero_local_container_preserves_local_fallback() {
    for container in [None, Some([0; 16])] {
        let mut keyboard = DeviceTree::keyboard(Some(2));
        keyboard.container_id = container;
        let tree = DeviceTree::new([
            (1, keyboard),
            (
                2,
                DeviceTree::product("USB\\VID_1234&PID_5678\\keyboard", container),
            ),
        ]);

        assert_eq!(
            DeviceName::lookup(&tree, 1).as_deref(),
            Some("HID Keyboard Device")
        );
    }
}

#[test]
fn stops_at_infrastructure_even_with_matching_container() {
    for instance in [
        "USB\\ROOT_HUB30\\hub",
        "usb\\root_hub\\hub",
        "PCI\\VEN_1234\\controller",
        "ACPI\\PNP0303\\keyboard",
        "ROOT\\device\\computer",
        "HTREE\\ROOT\\0",
    ] {
        let tree = DeviceTree::new([
            (1, DeviceTree::keyboard(Some(2))),
            (2, DeviceTree::product(instance, Some([1; 16]))),
        ]);

        assert_eq!(
            DeviceName::lookup(&tree, 1).as_deref(),
            Some("HID Keyboard Device"),
            "must stop before {instance}"
        );
    }
}

#[test]
fn missing_parent_identity_keeps_local_name() {
    let mut parent = DeviceTree::product("USB\\device", Some([1; 16]));
    parent.instance_id = None;
    let tree = DeviceTree::new([(1, DeviceTree::keyboard(Some(2))), (2, parent)]);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("HID Keyboard Device")
    );
}

#[test]
fn uses_nearest_friendly_name_when_bus_names_are_absent() {
    let mut parent = DeviceTree::keyboard(Some(3));
    parent.friendly_name = Some("Nearest Friendly Keyboard".to_owned());
    let mut grandparent = DeviceTree::keyboard(None);
    grandparent.friendly_name = Some("Distant Friendly Keyboard".to_owned());
    let tree = DeviceTree::new([
        (1, DeviceTree::keyboard(Some(2))),
        (2, parent),
        (3, grandparent),
    ]);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("Nearest Friendly Keyboard")
    );
}

#[test]
fn bus_product_above_friendly_name_takes_priority() {
    let mut keyboard = DeviceTree::keyboard(Some(2));
    keyboard.friendly_name = Some("Local Friendly Keyboard".to_owned());
    let tree = DeviceTree::new([
        (1, keyboard),
        (
            2,
            DeviceTree::product("USB\\VID_1234&PID_5678\\keyboard", Some([1; 16])),
        ),
    ]);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("Actual Keyboard Model")
    );
}

#[test]
fn blank_names_fall_back_to_original_description() {
    let mut keyboard = DeviceTree::keyboard(None);
    keyboard.bus_name = Some(" \t".to_owned());
    keyboard.friendly_name = Some(" ".to_owned());
    let tree = DeviceTree::new([(1, keyboard)]);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("HID Keyboard Device")
    );
    assert_eq!(DeviceName::lookup(&tree, 999), None);
}

#[test]
fn cycles_preserve_best_fallback() {
    let mut parent = DeviceTree::keyboard(Some(1));
    parent.friendly_name = Some("Friendly Keyboard".to_owned());
    let tree = DeviceTree::new([(1, DeviceTree::keyboard(Some(2))), (2, parent)]);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("Friendly Keyboard")
    );
}

#[test]
fn excessive_ancestry_cannot_choose_distant_product() {
    let mut nodes: Vec<(u32, DeviceNode)> = (1..64)
        .map(|id| (id, DeviceTree::keyboard(Some(id + 1))))
        .collect();
    nodes.push((64, DeviceTree::product("USB\\device", Some([1; 16]))));
    let tree = DeviceTree::new(nodes);

    assert_eq!(
        DeviceName::lookup(&tree, 1).as_deref(),
        Some("HID Keyboard Device")
    );
}
