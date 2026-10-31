//! Product-name selection independent of the Windows device-property APIs.

#[derive(Clone, Default)]
pub(super) struct DeviceNode {
    bus_name: Option<String>,
    friendly_name: Option<String>,
    description: Option<String>,
    container_id: Option<[u8; 16]>,
    instance_id: Option<String>,
    parent: Option<u32>,
}

impl DeviceNode {
    pub(super) fn new(
        bus_name: Option<String>,
        friendly_name: Option<String>,
        description: Option<String>,
        container_id: Option<[u8; 16]>,
        instance_id: Option<String>,
        parent: Option<u32>,
    ) -> Self {
        Self {
            bus_name,
            friendly_name,
            description,
            container_id,
            instance_id,
            parent,
        }
    }
}

pub(super) trait DeviceProperties {
    fn properties(&self, node: u32) -> DeviceNode;
}

pub(super) struct DeviceName;

impl DeviceName {
    pub(super) fn lookup(properties: &impl DeviceProperties, node: u32) -> Option<String> {
        let original = properties.properties(node);
        if let Some(name) = Self::nonempty(original.bus_name) {
            return Some(name);
        }
        let mut friendly_name = Self::nonempty(original.friendly_name);
        let description = Self::nonempty(original.description);
        let Some(container_id) = original.container_id.filter(|id| *id != [0; 16]) else {
            return friendly_name.or(description);
        };
        let mut parent = original.parent;
        let mut visited = vec![node];
        // A peripheral's product node is normally only one or two hops away.
        // Bound both work and allocation if device metadata is malformed.
        for _ in 0..16 {
            let Some(node) = parent else {
                break;
            };
            if visited.contains(&node) {
                break;
            }
            visited.push(node);
            let metadata = properties.properties(node);
            if metadata.container_id != Some(container_id)
                || !metadata
                    .instance_id
                    .as_deref()
                    .is_some_and(Self::is_peripheral)
            {
                break;
            }
            if let Some(name) = Self::nonempty(metadata.bus_name) {
                return Some(name);
            }
            if friendly_name.is_none() {
                friendly_name = Self::nonempty(metadata.friendly_name);
            }
            parent = metadata.parent;
        }
        friendly_name.or(description)
    }

    fn nonempty(value: Option<String>) -> Option<String> {
        value.and_then(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
    }

    fn is_peripheral(instance_id: &str) -> bool {
        let instance_id = instance_id.trim().to_ascii_uppercase();
        !instance_id.is_empty()
            && !["USB\\ROOT_HUB", "PCI\\", "ACPI\\", "ROOT\\", "HTREE\\"]
                .iter()
                .any(|prefix| instance_id.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests;
