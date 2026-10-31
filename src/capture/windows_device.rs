use std::{mem::size_of, ptr::null, ptr::null_mut};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Parent, SetupDiCreateDeviceInfoList,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiOpenDeviceInterfaceW,
    CR_BUFFER_SMALL, CR_SUCCESS, HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVINFO_DATA,
};
use windows_sys::Win32::Devices::Properties::{
    DEVPKEY_Device_BusReportedDeviceDesc, DEVPKEY_Device_ContainerId, DEVPKEY_Device_DeviceDesc,
    DEVPKEY_Device_FriendlyName, DEVPKEY_Device_InstanceId, DEVPROPTYPE, DEVPROP_TYPE_GUID,
    DEVPROP_TYPE_STRING,
};
use windows_sys::Win32::Foundation::DEVPROPKEY;

use super::device_names::{DeviceName, DeviceNode, DeviceProperties};

pub(super) struct DeviceInfo {
    handle: HDEVINFO,
}

impl DeviceInfo {
    pub(super) fn lookup_name(path: &str) -> Option<String> {
        let device = Self::open(path)?;
        let mut data = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..SP_DEVINFO_DATA::default()
        };
        // SAFETY: this set contains only the device added by open, and data is
        // writable with the structure size required by SetupAPI.
        if unsafe { SetupDiEnumDeviceInfo(device.handle, 0, &mut data) } == 0 {
            return None;
        }
        DeviceName::lookup(&device, data.DevInst)
    }

    fn open(path: &str) -> Option<Self> {
        // SAFETY: null class and parent create an empty local information set.
        let handle = unsafe { SetupDiCreateDeviceInfoList(null(), null_mut()) };
        if handle == -1 {
            return None;
        }
        let device = Self { handle };
        let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..SP_DEVICE_INTERFACE_DATA::default()
        };
        // SAFETY: path is terminated UTF-16, the set is live, and interface is
        // initialized. Opening adds the underlying device to this empty set.
        if unsafe { SetupDiOpenDeviceInterfaceW(handle, path.as_ptr(), 0, &mut interface) } == 0 {
            return None;
        }
        Some(device)
    }

    fn property(node: u32, key: &DEVPROPKEY, expected_type: DEVPROPTYPE) -> Option<Vec<u8>> {
        let mut bytes = 0;
        let mut property_type = 0;
        // SAFETY: key and outputs are valid; a null buffer with zero size
        // queries the byte count. Disconnected or absent properties are normal.
        let result = unsafe {
            CM_Get_DevNode_PropertyW(node, key, &mut property_type, null_mut(), &mut bytes, 0)
        };
        if ![CR_BUFFER_SMALL, CR_SUCCESS].contains(&result)
            || property_type != expected_type
            || bytes == 0
            || bytes > 64 * 1024
        {
            return None;
        }
        let mut value = vec![0_u8; bytes as usize];
        // SAFETY: the byte buffer is allocated for the supplied size. The API
        // returns a failure if the property grows or the device disconnects.
        let result = unsafe {
            CM_Get_DevNode_PropertyW(
                node,
                key,
                &mut property_type,
                value.as_mut_ptr(),
                &mut bytes,
                0,
            )
        };
        if result != CR_SUCCESS || property_type != expected_type || bytes as usize > value.len() {
            return None;
        }
        value.truncate(bytes as usize);
        Some(value)
    }

    fn string_property(node: u32, key: &DEVPROPKEY) -> Option<String> {
        let bytes = Self::property(node, key, DEVPROP_TYPE_STRING)?;
        if bytes.len() % size_of::<u16>() != 0 {
            return None;
        }
        let units: Vec<u16> = bytes
            .chunks_exact(size_of::<u16>())
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        if units.last() != Some(&0) {
            return None;
        }
        String::from_utf16(units.split(|unit| *unit == 0).next()?).ok()
    }
}

impl DeviceProperties for DeviceInfo {
    fn properties(&self, node: u32) -> DeviceNode {
        let mut parent = 0;
        // SAFETY: parent is writable; the devnode came from SetupAPI or an
        // earlier successful parent query. A disconnect returns an error.
        let parent =
            (unsafe { CM_Get_Parent(&mut parent, node, 0) } == CR_SUCCESS).then_some(parent);
        DeviceNode::new(
            Self::string_property(node, &DEVPKEY_Device_BusReportedDeviceDesc),
            Self::string_property(node, &DEVPKEY_Device_FriendlyName),
            Self::string_property(node, &DEVPKEY_Device_DeviceDesc),
            Self::property(node, &DEVPKEY_Device_ContainerId, DEVPROP_TYPE_GUID)
                .and_then(|bytes| bytes.try_into().ok()),
            Self::string_property(node, &DEVPKEY_Device_InstanceId),
            parent,
        )
    }
}

impl Drop for DeviceInfo {
    fn drop(&mut self) {
        // SAFETY: this instance uniquely owns a successful SetupAPI allocation.
        unsafe { SetupDiDestroyDeviceInfoList(self.handle) };
    }
}
