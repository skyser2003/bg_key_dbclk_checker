//! Passive global keyboard capture through a Quartz session event tap.
//!
//! Quartz does not expose individual keyboard identity, so this adapter reports
//! the combined physical keyboard stream. It never changes or consumes input.

use crate::event::{CaptureMessage, Key, KeyAction, KeyEvent, Source};
use crate::macos_keys::{self, Action};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{Duration, Utc};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;

type CFObject = *mut c_void;
type CGEvent = *mut c_void;

const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
const SYSTEM_DEFINED: u32 = 14;
const TAP_DISABLED_TIMEOUT: u32 = 0xffff_fffe;
const TAP_DISABLED_USER_INPUT: u32 = 0xffff_ffff;
const EVENT_KEYCODE: u32 = 9;
const EVENT_AUTOREPEAT: u32 = 8;
const EVENT_SOURCE_PID: u32 = 41;
const EVENT_SOURCE_STATE: u32 = 45;
const HID_SYSTEM_STATE: i32 = 1;

#[repr(C)]
#[derive(Default)]
struct Timebase {
    numer: u32,
    denom: u32,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        events: u64,
        callback: unsafe extern "C" fn(CFObject, u32, CGEvent, *mut c_void) -> CGEvent,
        user_info: *mut c_void,
    ) -> CFObject;
    fn CGEventTapEnable(tap: CFObject, enable: bool);
    fn CGEventTapIsEnabled(tap: CFObject) -> bool;
    fn CGEventGetIntegerValueField(event: CGEvent, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEvent) -> u64;
    fn CGEventGetTimestamp(event: CGEvent) -> u64;
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFObject,
        order: isize,
    ) -> CFObject;
    fn CFMachPortInvalidate(port: CFObject);
    fn CFMachPortIsValid(port: CFObject) -> u8;
    fn CFRunLoopSourceInvalidate(source: CFObject);
    fn CFRunLoopGetCurrent() -> CFObject;
    fn CFRunLoopAddSource(run_loop: CFObject, source: CFObject, mode: *const c_void);
    fn CFRunLoopRemoveSource(run_loop: CFObject, source: CFObject, mode: *const c_void);
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    fn CFRelease(object: *const c_void);
}

#[link(name = "System")]
unsafe extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> i32;
    fn mach_absolute_time() -> u64;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    // Referencing the class symbol also ensures AppKit is loaded. Auxiliary
    // keyboard payloads have no public CGEvent getters, but NSEvent exposes them.
    #[link_name = "OBJC_CLASS_$_NSEvent"]
    static NSEVENT_CLASS: c_void;
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_msgSend();
    fn sel_registerName(name: *const std::ffi::c_char) -> CFObject;
    fn objc_autoreleasePoolPush() -> CFObject;
    fn objc_autoreleasePoolPop(pool: CFObject);
}

struct AutoreleasePool(CFObject);

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        // SAFETY: This token was created by Push on this thread and is popped
        // exactly once, after the temporary NSEvent has been fully inspected.
        unsafe { objc_autoreleasePoolPop(self.0) };
    }
}

fn auxiliary_key(event: CGEvent) -> Option<(Key, Action)> {
    // SAFETY: The callback lends a live CGEvent. The Objective-C messages below
    // have the exact NSEvent.h signatures: +eventWithCGEvent: -> id,
    // -subtype -> short, and -data1 -> NSInteger. Neither x86_64 nor arm64 needs
    // objc_msgSend_stret for these scalar/pointer results. The local pool frees
    // the autoreleased bridge object without leaking one object per key event.
    unsafe {
        let _pool = AutoreleasePool(objc_autoreleasePoolPush());
        let create = std::mem::transmute::<
            unsafe extern "C" fn(),
            unsafe extern "C" fn(CFObject, CFObject, CGEvent) -> CFObject,
        >(objc_msgSend);
        let subtype = std::mem::transmute::<
            unsafe extern "C" fn(),
            unsafe extern "C" fn(CFObject, CFObject) -> i16,
        >(objc_msgSend);
        let data1 = std::mem::transmute::<
            unsafe extern "C" fn(),
            unsafe extern "C" fn(CFObject, CFObject) -> isize,
        >(objc_msgSend);
        let native = create(
            ptr::addr_of!(NSEVENT_CLASS).cast_mut(),
            sel_registerName(c"eventWithCGEvent:".as_ptr()),
            event,
        );
        if native.is_null() {
            return None;
        }
        let subtype = subtype(native, sel_registerName(c"subtype".as_ptr()));
        let data1 = data1(native, sel_registerName(c"data1".as_ptr()));
        macos_keys::auxiliary_key(subtype, data1 as i64)
            .map(|(code, name, action)| (Key { code, name }, action))
    }
}

struct TapResources {
    tap: CFObject,
    source: CFObject,
    run_loop: CFObject,
}

impl Drop for TapResources {
    fn drop(&mut self) {
        // SAFETY: We own the Create-rule references to tap/source. They stay
        // valid until this cleanup on the same thread as their run loop. Remove
        // and invalidate first, so no callback can reference the context after
        // run() drops it. The current run loop is borrowed and never released.
        unsafe {
            CGEventTapEnable(self.tap, false);
            if !self.source.is_null() {
                CFRunLoopRemoveSource(self.run_loop, self.source, kCFRunLoopDefaultMode);
                CFRunLoopSourceInvalidate(self.source);
                CFRelease(self.source);
            }
            CFMachPortInvalidate(self.tap);
            CFRelease(self.tap);
        }
    }
}

struct CaptureContext {
    tx: SyncSender<CaptureMessage>,
    source: Source,
    tap: CFObject,
    timebase: Timebase,
    failure: Option<anyhow::Error>,
}

impl CaptureContext {
    fn send(&self, message: CaptureMessage) -> Result<()> {
        match self.tx.try_send(message) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => bail!(
                "macOS keyboard capture queue overflowed; stopping because key transitions were lost"
            ),
            Err(TrySendError::Disconnected(_)) => bail!("keyboard event receiver disconnected"),
        }
    }

    fn seed_held_keys(&self) -> Result<()> {
        let mut held_keys = Vec::new();
        // SAFETY: CGEventSourceKeyState accepts a HID state and native virtual
        // key code; no object ownership or callback state is involved. Quartz
        // keyboard virtual key codes occupy this 8-bit range.
        for code in 0..=255_u16 {
            if unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, code) } {
                held_keys.push(Key {
                    code: u32::from(code),
                    name: macos_keys::key_name(code),
                });
            }
        }
        self.send(CaptureMessage::Reset {
            source: self.source.clone(),
            held_keys,
        })
    }

    fn receive(&mut self, event_type: u32, event: CGEvent) -> Result<()> {
        if self.failure.is_some() {
            return Ok(());
        }
        if matches!(event_type, TAP_DISABLED_TIMEOUT | TAP_DISABLED_USER_INPUT) {
            // A disabled tap is an input gap, so seed current physical state
            // and discard all earlier press history before accepting more keys.
            self.seed_held_keys()?;
            // SAFETY: tap is owned by run() and callbacks execute only while
            // its run-loop source is live; re-enabling is documented by Quartz.
            unsafe { CGEventTapEnable(self.tap, true) };
            if !unsafe { CGEventTapIsEnabled(self.tap) } {
                bail!("macOS keyboard event tap could not resume; check Input Monitoring/Accessibility permission and restart monitoring");
            }
            return Ok(());
        }
        if event.is_null() {
            bail!("macOS keyboard event tap returned a null keyboard event");
        }
        // SAFETY: For ordinary callback types Quartz lends a non-null CGEvent
        // valid until this callback returns. These getters do not mutate it.
        let (source_pid, source_state, flags, autorepeat, event_ns) = unsafe {
            (
                CGEventGetIntegerValueField(event, EVENT_SOURCE_PID),
                CGEventGetIntegerValueField(event, EVENT_SOURCE_STATE),
                CGEventGetFlags(event),
                CGEventGetIntegerValueField(event, EVENT_AUTOREPEAT) != 0,
                CGEventGetTimestamp(event),
            )
        };
        if macos_keys::is_software_posted(source_pid, source_state) {
            return Ok(());
        }
        let normalized = if event_type == SYSTEM_DEFINED {
            auxiliary_key(event)
        } else {
            // SAFETY: The callback event is still live; this getter reads its
            // keyboard virtual code without modifying or retaining the event.
            let native_code = unsafe { CGEventGetIntegerValueField(event, EVENT_KEYCODE) };
            let code = u16::try_from(native_code)
                .context("macOS returned an invalid keyboard key code")?;
            macos_keys::action(event_type, code, flags, autorepeat).map(|action| {
                (
                    Key {
                        code: u32::from(code),
                        name: macos_keys::key_name(code),
                    },
                    action,
                )
            })
        };
        let Some((key, action)) = normalized else {
            return Ok(());
        };
        // Explicit autorepeat is omitted before the detector and opt-in log.
        if action == Action::Repeat || (action == Action::Press && autorepeat) {
            return Ok(());
        }
        // CGEvent timestamps are nanoseconds of mach absolute uptime. Convert
        // the callback's current uptime only to reconstruct the event's UTC
        // time; use the untouched source timestamp for all detector intervals.
        // SAFETY: mach_absolute_time reads an OS clock without object pointers.
        let now_ns = macos_keys::ticks_to_nanoseconds(
            unsafe { mach_absolute_time() },
            self.timebase.numer,
            self.timebase.denom,
        )
        .context("macOS monotonic clock conversion overflowed")?;
        let now_utc = Utc::now();
        let offset = macos_keys::timestamp_offset_ns(event_ns, now_ns)
            .context("macOS keyboard event timestamp is out of range")?;
        let timestamp = now_utc
            .checked_add_signed(Duration::nanoseconds(offset))
            .context("macOS keyboard event UTC timestamp is out of range")?;
        self.send(CaptureMessage::Event(KeyEvent {
            source: self.source.clone(),
            key,
            action: match action {
                Action::Press => KeyAction::Press,
                Action::Release => KeyAction::Release,
                Action::Repeat => KeyAction::Repeat,
            },
            monotonic_ns: event_ns,
            timestamp,
        }))
    }
}

unsafe extern "C" fn event_callback(
    _proxy: CFObject,
    event_type: u32,
    event: CGEvent,
    user_info: *mut c_void,
) -> CGEvent {
    // SAFETY: run() gives Quartz the stable address of a boxed context, which
    // lives longer than the tap source. Only the owning thread runs callbacks,
    // and it accesses the box only between CFRunLoopRunInMode calls.
    let context = unsafe { &mut *user_info.cast::<CaptureContext>() };
    let outcome = catch_unwind(AssertUnwindSafe(|| context.receive(event_type, event)));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => context.failure = Some(error),
        Err(_) => context.failure = Some(anyhow!("macOS keyboard event callback panicked")),
    }
    // The tap is listen-only. Always return the original event unchanged.
    event
}

pub fn run(tx: SyncSender<CaptureMessage>, stop: Arc<AtomicBool>) -> Result<()> {
    let mut timebase = Timebase::default();
    // SAFETY: timebase points to the correctly laid-out writable Mach struct.
    if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
        bail!("cannot initialize the macOS monotonic clock");
    }
    let mut context = Box::new(CaptureContext {
        tx,
        source: Source {
            id: "macos:combined".to_owned(),
            name: Some("Combined keyboard stream".to_owned()),
        },
        tap: ptr::null_mut(),
        timebase,
        failure: None,
    });
    let event_mask = (1_u64 << KEY_DOWN)
        | (1_u64 << KEY_UP)
        | (1_u64 << FLAGS_CHANGED)
        | (1_u64 << SYSTEM_DEFINED);
    // SAFETY: The callback and its context have the declared C ABI and lifetime.
    // Session location (1) avoids the root-only HID tap; head placement (0) and
    // listen-only option (1) observe keys without suppressing or modifying them.
    let tap = unsafe {
        CGEventTapCreate(
            1,
            0,
            1,
            event_mask,
            event_callback,
            (&mut *context as *mut CaptureContext).cast(),
        )
    };
    if tap.is_null() {
        bail!("cannot create a macOS keyboard event tap. Open System Settings > Privacy & Security > Input Monitoring and allow this executable or the terminal that launches it, then restart it. If capture is still denied, also allow Accessibility access. Run monitoring in a logged-in desktop session");
    }
    context.tap = tap;
    // SAFETY: The tap is a valid owned CFMachPort; default allocator is null.
    // CFRunLoopGetCurrent returns a borrowed loop belonging to this thread.
    let mut resources = TapResources {
        tap,
        source: ptr::null_mut(),
        run_loop: unsafe { CFRunLoopGetCurrent() },
    };
    resources.source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
    if resources.source.is_null() {
        bail!("cannot create the macOS keyboard event tap run-loop source");
    }
    context.seed_held_keys()?;
    // SAFETY: The live owned source is added to its owning thread's borrowed
    // run loop; cleanup removes it before the callback context is destroyed.
    unsafe { CFRunLoopAddSource(resources.run_loop, resources.source, kCFRunLoopDefaultMode) };
    while !stop.load(Ordering::Relaxed) {
        // SAFETY: The mode is a CoreFoundation constant. Run for at most 50 ms
        // or until an event is handled, so Ctrl+C stops even with no key input.
        let outcome = unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 1) };
        if let Some(error) = context.failure.take() {
            return Err(error);
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // SAFETY: resources owns the tap until the loop returns and Drop runs.
        if outcome == 1 || outcome == 2 || unsafe { CFMachPortIsValid(tap) } == 0 {
            bail!("macOS keyboard event tap stopped unexpectedly; check Input Monitoring/Accessibility permission and restart monitoring");
        }
    }
    Ok(())
}
