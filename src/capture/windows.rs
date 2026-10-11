//! Global hardware keyboard capture through Windows Raw Input.
//!
//! This registration observes background input without RIDEV_NOLEGACY or a
//! keyboard hook, so it does not suppress normal keyboard operation.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io;
use std::mem::{offset_of, size_of};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use windows_sys::Win32::Foundation::{HANDLE, HINSTANCE, HWND, WAIT_FAILED};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyNameTextW, MapVirtualKeyW, MAPVK_VK_TO_VSC_EX,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices,
    RAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RAWKEYBOARD, RIDEV_DEVNOTIFY,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RIDI_DEVICENAME, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MsgWaitForMultipleObjects,
    PeekMessageW, RegisterClassW, UnregisterClassW, GIDC_ARRIVAL, GIDC_REMOVAL, HWND_MESSAGE, MSG,
    PM_REMOVE, QS_ALLINPUT, WM_INPUT, WM_INPUT_DEVICE_CHANGE, WM_QUIT, WNDCLASSW,
};

use super::windows_keys::{
    canonical_virtual_key, event_timestamp, message_ticks, physical_code, KeyState,
    PacketNormalizer, PAUSE_CODE,
};
use crate::event::{CaptureMessage, Key, KeyEvent, Source};

static WINDOW_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Capture on a dedicated thread until `stop` is set. The 50 ms message wait
/// ensures an idle keyboard cannot prevent Ctrl+C shutdown.
pub fn run(tx: SyncSender<CaptureMessage>, stop: Arc<AtomicBool>) -> Result<()> {
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }

    // A global snapshot is all Raw Input exposes at startup. Seed every initial
    // keyboard conservatively; a release from that keyboard removes its guard.
    let window =
        InputWindow::create().context("could not create Windows keyboard capture window")?;
    // Register first, so every transition during the snapshot has a queued
    // message to update the initial state before normal recording begins.
    let startup_held = held_virtual_keys();
    let mut state = CaptureState {
        tx,
        devices: HashMap::new(),
    };
    for handle in keyboard_devices().context("could not enumerate Windows keyboards")? {
        state.add_device(handle, startup_held.clone())?;
    }

    while !stop.load(Ordering::Acquire) {
        let mut message = MSG::default();
        // SAFETY: message points to an initialized writable MSG; null HWND
        // selects this capture thread's message queue.
        while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if stop.load(Ordering::Acquire) {
                // WM_INPUT still needs DefWindowProc cleanup if shutdown won
                // the race after removing this message from the queue.
                unsafe { DispatchMessageW(&message) };
                return Ok(());
            }
            if message.message == WM_QUIT {
                bail!("Windows keyboard capture message loop stopped unexpectedly");
            }
            let handled = match message.message {
                WM_INPUT => state.input(message.lParam as _, message.time),
                WM_INPUT_DEVICE_CHANGE => {
                    state.device_change(message.wParam as u32, message.lParam as HANDLE)
                }
                _ => Ok(()),
            };
            // SAFETY: MSG was returned by PeekMessageW. The class uses the
            // native DefWindowProcW, which also releases foreground WM_INPUT
            // resources. Dispatch even when parsing/sending returned an error.
            unsafe { DispatchMessageW(&message) };
            handled?;
        }
        // SAFETY: zero handles permits a null array. Wake for queued input or
        // after 50 ms to check the shared stop flag without busy waiting.
        if unsafe { MsgWaitForMultipleObjects(0, null(), 0, 50, QS_ALLINPUT) } == WAIT_FAILED {
            return Err(io::Error::last_os_error()).context("Windows keyboard message wait failed");
        }
    }
    // Keep the window and Raw Input registration alive through the whole loop.
    drop(window);
    Ok(())
}

struct Device {
    source: Source,
    keys: KeyState,
    names: HashMap<u32, String>,
    startup_cutoff: u64,
    packets: PacketNormalizer,
}

struct CaptureState {
    tx: SyncSender<CaptureMessage>,
    devices: HashMap<usize, Device>,
}

impl CaptureState {
    fn send(&self, message: CaptureMessage) -> Result<()> {
        match self.tx.try_send(message) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => bail!(
                "keyboard capture queue overflowed; stopping because events were lost (storage may be too slow)"
            ),
            Err(TrySendError::Disconnected(_)) => {
                bail!("keyboard capture consumer disconnected")
            }
        }
    }

    fn add_device(&mut self, handle: HANDLE, held: HashSet<u16>) -> Result<()> {
        let source = device_source(handle);
        self.send(CaptureMessage::Reset {
            source: source.clone(),
            // Startup-held virtual keys are guarded by KeyState because their
            // physical scan codes cannot be queried per individual keyboard.
            held_keys: Vec::new(),
        })?;
        self.devices.insert(
            handle as usize,
            Device {
                source,
                keys: startup_key_state(held),
                names: HashMap::new(),
                // Events at/before this initialization cutoff only seed held
                // state; they cannot be treated as new physical presses.
                startup_cutoff: unsafe { GetTickCount64() },
                packets: PacketNormalizer::default(),
            },
        );
        Ok(())
    }

    fn device_change(&mut self, change: u32, handle: HANDLE) -> Result<()> {
        if change == GIDC_REMOVAL {
            if let Some(device) = self.devices.remove(&(handle as usize)) {
                self.send(CaptureMessage::Reset {
                    source: device.source,
                    held_keys: Vec::new(),
                })?;
            }
        } else if change == GIDC_ARRIVAL {
            // Ignore mouse/HID notifications. Enumeration also handles an
            // arrival which raced with our initial enumeration.
            if keyboard_devices()?.contains(&handle) {
                if let Some(old) = self.devices.remove(&(handle as usize)) {
                    self.send(CaptureMessage::Reset {
                        source: old.source,
                        held_keys: Vec::new(),
                    })?;
                }
                self.add_device(handle, held_virtual_keys())?;
            }
        }
        Ok(())
    }

    fn input(&mut self, handle: *mut c_void, native_time: u32) -> Result<()> {
        // Obtain timing before identity lookup, key naming, or queueing. MSG.time
        // was assigned by Windows when the input message entered this queue.
        let current_ticks = unsafe { GetTickCount64() };
        let current_wall = Utc::now();
        let ticks = message_ticks(native_time, current_ticks);
        let raw = read_raw_keyboard(handle)?;
        let Some((device_handle, keyboard)) = raw else {
            return Ok(());
        };
        if keyboard.MakeCode == 0xff {
            bail!("Windows keyboard reported an input overrun; stopping because hardware events were lost");
        }
        if !self.devices.contains_key(&(device_handle as usize)) {
            // A keyboard can deliver input before its arrival notification.
            // No prior history is reused, even if Windows recycled the handle.
            self.add_device(device_handle, HashSet::new())?;
        }
        let device = self
            .devices
            .get_mut(&(device_handle as usize))
            .ok_or_else(|| anyhow!("Windows keyboard disappeared while processing its input"))?;
        let Some(code) = device
            .packets
            .normalize(keyboard.MakeCode, keyboard.Flags, keyboard.VKey)
        else {
            return Ok(());
        };
        let action = device.keys.classify_at_time(
            code,
            if code == PAUSE_CODE {
                0x13
            } else {
                canonical_virtual_key(keyboard.MakeCode, keyboard.Flags, keyboard.VKey)
            },
            keyboard.Flags,
            ticks,
            device.startup_cutoff,
        );
        let name = device
            .names
            .entry(code)
            .or_insert_with(|| {
                if code == PAUSE_CODE {
                    "Pause".to_owned()
                } else {
                    key_name(keyboard)
                }
            })
            .clone();
        let event = KeyEvent {
            source: device.source.clone(),
            key: Key { code, name },
            action,
            monotonic_ns: ticks.saturating_mul(1_000_000),
            timestamp: event_timestamp(ticks, current_ticks, current_wall)
                .context("Windows keyboard UTC timestamp is out of range")?,
        };
        self.send(CaptureMessage::Event(event))
    }
}

/// Own the window, window class, and Raw Input registration. Their teardown is
/// guaranteed on every return/error path, on the same thread that created them.
struct InputWindow {
    handle: HWND,
    instance: HINSTANCE,
    class_name: Vec<u16>,
    registered: bool,
}

impl InputWindow {
    fn create() -> Result<Self> {
        // SAFETY: null requests the module handle of the current executable.
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return Err(io::Error::last_os_error()).context("GetModuleHandleW failed");
        }
        let class_name: Vec<u16> = format!(
            "bg_key_dbclk_checker_raw_input_{}",
            WINDOW_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let class = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..WNDCLASSW::default()
        };
        // SAFETY: the class and its null-terminated name remain valid for the
        // call; the name stays owned until UnregisterClassW in Drop.
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(io::Error::last_os_error()).context("RegisterClassW failed");
        }
        let mut window = Self {
            handle: null_mut(),
            instance,
            class_name,
            registered: false,
        };
        // SAFETY: class name refers to the registered class. HWND_MESSAGE makes
        // a hidden message-only window, with no user state or owning parent.
        window.handle = unsafe {
            CreateWindowExW(
                0,
                window.class_name.as_ptr(),
                window.class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                instance,
                null(),
            )
        };
        if window.handle.is_null() {
            return Err(io::Error::last_os_error()).context("CreateWindowExW failed");
        }
        let registration = RAWINPUTDEVICE {
            usUsagePage: 1,
            usUsage: 6,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: window.handle,
        };
        // SAFETY: registration points to one initialized keyboard descriptor.
        if unsafe { RegisterRawInputDevices(&registration, 1, size_of::<RAWINPUTDEVICE>() as u32) }
            == 0
        {
            return Err(io::Error::last_os_error())
                .context("registering background keyboard Raw Input failed");
        }
        window.registered = true;
        Ok(window)
    }
}

impl Drop for InputWindow {
    fn drop(&mut self) {
        // SAFETY: all these resources were created on the current thread. Remove
        // must use a null target; class name remains alive for the full teardown.
        unsafe {
            if self.registered {
                let registration = RAWINPUTDEVICE {
                    usUsagePage: 1,
                    usUsage: 6,
                    dwFlags: RIDEV_REMOVE,
                    hwndTarget: null_mut(),
                };
                RegisterRawInputDevices(&registration, 1, size_of::<RAWINPUTDEVICE>() as u32);
            }
            if !self.handle.is_null() {
                DestroyWindow(self.handle);
            }
            UnregisterClassW(self.class_name.as_ptr(), self.instance);
        }
    }
}

fn held_virtual_keys() -> HashSet<u16> {
    (1..=254)
        // The sided modifier states are available separately. Generic Shift,
        // Control, and Alt would otherwise seed the opposite modifier side.
        .filter(|key| !matches!(key, 0x10..=0x12))
        // SAFETY: each argument is a valid Windows virtual key value. Only the
        // high bit reports currently-held keys; the unreliable low bit is unused.
        .filter(|key| unsafe { GetAsyncKeyState(i32::from(*key)) } < 0)
        .collect()
}

fn startup_key_state(held: HashSet<u16>) -> KeyState {
    let mut keys = KeyState::new(held.clone());
    for virtual_key in held {
        // SAFETY: virtual_key is from GetAsyncKeyState's valid 1..=254 range.
        // This mapping is captured once, before later layout changes can alter
        // the virtual identity of a key which is already physically held.
        let mapped = unsafe { MapVirtualKeyW(u32::from(virtual_key), MAPVK_VK_TO_VSC_EX) };
        let make_code = (mapped & 0xff) as u16;
        let flags = match (mapped >> 8) & 0xff {
            0xe0 => 2,
            0xe1 => 4,
            _ => 0,
        };
        if let Some(code) = physical_code(make_code, flags, virtual_key) {
            keys.guard_startup_physical_key(code, virtual_key);
        }
        // GetAsyncKeyState merges main/keypad Enter and keypad navigation keys
        // (with Num Lock off). Guard both possible scan-code extension forms.
        if matches!(virtual_key, 0x0c | 0x0d | 0x21..=0x28 | 0x2d | 0x2e) && make_code != 0 {
            for flags in [0, 2] {
                if let Some(code) = physical_code(make_code, flags, virtual_key) {
                    keys.guard_startup_physical_key(code, virtual_key);
                }
            }
        }
    }
    keys
}

fn keyboard_devices() -> Result<Vec<HANDLE>> {
    // A device can appear between count and fill. Retry the size query instead
    // of failing on an ordinary hotplug race.
    for _ in 0..4 {
        let mut count = 0;
        // SAFETY: null array requests count; count is a writable u32.
        if unsafe {
            GetRawInputDeviceList(
                null_mut(),
                &mut count,
                size_of::<RAWINPUTDEVICELIST>() as u32,
            )
        } == u32::MAX
        {
            return Err(io::Error::last_os_error()).context("GetRawInputDeviceList count failed");
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
        // SAFETY: count matches the initialized array's capacity in elements.
        let read = unsafe {
            GetRawInputDeviceList(
                devices.as_mut_ptr(),
                &mut count,
                size_of::<RAWINPUTDEVICELIST>() as u32,
            )
        };
        if read == u32::MAX {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(122) {
                continue;
            }
            return Err(error).context("GetRawInputDeviceList failed");
        }
        return Ok(devices
            .into_iter()
            .take(read as usize)
            .filter(|device| device.dwType == RIM_TYPEKEYBOARD)
            .map(|device| device.hDevice)
            .collect());
    }
    bail!("Windows keyboard devices changed repeatedly during enumeration; retry monitoring")
}

fn device_source(handle: HANDLE) -> Source {
    let path = device_path(handle);
    Source {
        id: path
            .as_ref()
            .map(|path| format!("rawinput:{path}"))
            .unwrap_or_else(|| format!("rawinput:handle:{:x}", handle as usize)),
        name: path,
    }
}

fn device_path(handle: HANDLE) -> Option<String> {
    let mut length = 0;
    // SAFETY: null buffer queries a UTF-16 character count, not a byte count.
    if unsafe { GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, null_mut(), &mut length) }
        == u32::MAX
        || length == 0
    {
        return None;
    }
    let mut path = vec![0_u16; length as usize];
    // SAFETY: buffer has the requested UTF-16 capacity; the device may have
    // disconnected since lookup, in which case the fallback handle remains valid.
    let read = unsafe {
        GetRawInputDeviceInfoW(
            handle,
            RIDI_DEVICENAME,
            path.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if read == u32::MAX {
        return None;
    }
    let end = path
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(path.len());
    let path = String::from_utf16_lossy(&path[..end]);
    (!path.is_empty()).then_some(path)
}

fn read_raw_keyboard(handle: *mut c_void) -> Result<Option<(HANDLE, RAWKEYBOARD)>> {
    let mut size = 0;
    let header_size = size_of::<RAWINPUTHEADER>() as u32;
    // SAFETY: this WM_INPUT handle is valid until DefWindowProc cleanup, and the
    // null buffer requests its size.
    if unsafe { GetRawInputData(handle, RID_INPUT, null_mut(), &mut size, header_size) } == u32::MAX
    {
        return Err(io::Error::last_os_error()).context("reading Windows Raw Input size failed");
    }
    if (size as usize) < size_of::<RAWINPUTHEADER>() {
        bail!("Windows returned a truncated Raw Input header");
    }
    // Allocate machine words for RAWINPUT alignment, with enough initialized
    // padding for the full union even when only keyboard data is supplied.
    let bytes = (size as usize).max(size_of::<RAWINPUT>());
    let mut buffer = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
    // SAFETY: buffer is suitably aligned and at least size bytes long.
    let read = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            buffer.as_mut_ptr().cast(),
            &mut size,
            header_size,
        )
    };
    if read == u32::MAX {
        return Err(io::Error::last_os_error()).context("reading Windows Raw Input failed");
    }
    if (read as usize) < size_of::<RAWINPUTHEADER>() {
        bail!("Windows returned incomplete Raw Input data");
    }
    // SAFETY: initialized aligned allocation covers the entire RAWINPUT; its
    // native header fields contain no invalid Rust bit patterns.
    let raw = unsafe { &*buffer.as_ptr().cast::<RAWINPUT>() };
    if raw.header.dwType != RIM_TYPEKEYBOARD {
        return Ok(None);
    }
    if (read as usize) < offset_of!(RAWINPUT, data) + size_of::<RAWKEYBOARD>() {
        bail!("Windows returned a truncated Raw Input keyboard event");
    }
    // A null source may be used for software/remote input. It cannot identify a
    // physical keyboard, so it is outside the hardware stream we monitor.
    if raw.header.hDevice.is_null() {
        return Ok(None);
    }
    // SAFETY: dwType was checked to select the keyboard union member and the
    // returned byte count covers every field of RAWKEYBOARD.
    Ok(Some((raw.header.hDevice, unsafe { raw.data.keyboard })))
}

fn key_name(keyboard: RAWKEYBOARD) -> String {
    if keyboard.VKey == 0x13 {
        return "Pause".to_owned();
    }
    if keyboard.MakeCode == 0 {
        return match keyboard.VKey {
            0xad => "VolumeMute".to_owned(),
            0xae => "VolumeDown".to_owned(),
            0xaf => "VolumeUp".to_owned(),
            0xb0 => "MediaNextTrack".to_owned(),
            0xb1 => "MediaPreviousTrack".to_owned(),
            0xb2 => "MediaStop".to_owned(),
            0xb3 => "MediaPlayPause".to_owned(),
            value => format!("VirtualKey 0x{value:02x}"),
        };
    }
    let parameter =
        (i32::from(keyboard.MakeCode) << 16) | if keyboard.Flags & 2 != 0 { 1 << 24 } else { 0 };
    let mut name = [0_u16; 128];
    // SAFETY: name is a writable UTF-16 buffer with the advertised capacity;
    // parameter encodes the documented scan-code and extended-key fields.
    let length = unsafe { GetKeyNameTextW(parameter, name.as_mut_ptr(), name.len() as i32) };
    if length > 0 {
        String::from_utf16_lossy(&name[..length as usize])
    } else {
        format!(
            "Scan 0x{:03x}",
            physical_code(keyboard.MakeCode, keyboard.Flags, keyboard.VKey).unwrap_or(0)
        )
    }
}
