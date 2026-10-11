use crate::event::{CaptureMessage, Key, KeyAction, KeyEvent, Source};
use anyhow::{Context, Result};
use evdev::{raw_stream::RawDevice, EventSummary, KeyCode, SynchronizationCode};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc,
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

struct Keyboard {
    device: RawDevice,
    source: Source,
    dropped: bool,
}

fn key(code: KeyCode) -> Key {
    Key {
        code: u32::from(code.0),
        name: format!("{code:?}"),
    }
}
fn is_keyboard_key(code: KeyCode) -> bool {
    let c = code.0;
    c > 0 && (c < 0x100 || (0x160..=0x1ff).contains(&c) || (0x230..0x2c0).contains(&c))
}
fn action(value: i32) -> Option<KeyAction> {
    match value {
        0 => Some(KeyAction::Release),
        1 => Some(KeyAction::Press),
        2 => Some(KeyAction::Repeat),
        _ => None,
    }
}

fn send(tx: &SyncSender<CaptureMessage>, message: CaptureMessage) -> Result<()> {
    tx.try_send(message).context(
        "capture queue overflow or consumer closed; monitoring stopped to avoid silent input loss",
    )
}

fn held(device: &RawDevice) -> Result<Vec<Key>> {
    Ok(device
        .get_key_state()?
        .iter()
        .filter(|code| is_keyboard_key(*code))
        .map(key)
        .collect())
}

fn monotonic_now() -> Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: time is valid writable storage and CLOCK_MONOTONIC is a supported clock.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error()).context("cannot read monotonic clock");
    }
    Ok(time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64)
}

fn scan(
    devices: &mut BTreeMap<PathBuf, Keyboard>,
    warned: &mut HashSet<PathBuf>,
    tx: &SyncSender<CaptureMessage>,
) -> Result<()> {
    let entries=fs::read_dir("/dev/input").context("cannot inspect /dev/input; Linux capture needs readable keyboard evdev devices (X11 or Wayland)")?;
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.starts_with("event") || devices.contains_key(&path) {
            continue;
        }
        let physical = fs::canonicalize(Path::new("/sys/class/input").join(name).join("device"));
        match physical {
            Ok(p) if p.starts_with("/sys/devices/virtual/input") => continue,
            Ok(_) => {}
            Err(error) => {
                if warned.insert(path.clone()) {
                    eprintln!(
                        "Skipping {}: cannot identify a physical input device: {error}",
                        path.display()
                    );
                }
                continue;
            }
        }
        let opened = (|| -> Result<RawDevice> {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)?;
            Ok(RawDevice::from_fd(file.into())?)
        })();
        let device = match opened {
            Ok(device) => device,
            Err(error) => {
                if warned.insert(path.clone()) {
                    eprintln!("Skipping inaccessible input device {}: {error:#}. Capture coverage may be partial; grant read access to the required keyboard device.",path.display());
                }
                continue;
            }
        };
        if !device
            .supported_keys()
            .is_some_and(|keys| keys.iter().any(is_keyboard_key))
        {
            continue;
        }
        let clock = libc::CLOCK_MONOTONIC;
        // SAFETY: EVIOCSCLOCKID is _IOW('E', 0xa0, int); the fd is an open evdev
        // device and clock points to a live integer. This changes only this fd's timestamps.
        let result =
            unsafe { libc::ioctl(device.as_raw_fd(), 0x400445a0 as libc::c_ulong, &clock) };
        if result < 0 {
            return Err(std::io::Error::last_os_error())
                .context("cannot select evdev monotonic timestamps");
        }
        let source = Source {
            id: path.display().to_string(),
            name: device.name().map(str::to_owned),
        };
        send(
            tx,
            CaptureMessage::Reset {
                source: source.clone(),
                held_keys: held(&device)?,
            },
        )?;
        eprintln!(
            "Monitoring {} ({:?})",
            path.display(),
            source.name.as_deref().unwrap_or("keyboard")
        );
        devices.insert(
            path,
            Keyboard {
                device,
                source,
                dropped: false,
            },
        );
    }
    Ok(())
}

pub fn run(tx: SyncSender<CaptureMessage>, stop: Arc<AtomicBool>) -> Result<()> {
    let mut devices = BTreeMap::new();
    let mut warned = HashSet::new();
    scan(&mut devices, &mut warned, &tx)?;
    anyhow::ensure!(!devices.is_empty(),"no accessible physical keyboards; grant read access to keyboard /dev/input/event* devices, then restart (see README Linux permissions)");
    let mut last_scan = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if last_scan.elapsed() >= Duration::from_secs(1) {
            scan(&mut devices, &mut warned, &tx)?;
            last_scan = Instant::now();
        }
        let mut polls: Vec<_> = devices
            .values()
            .map(|k| libc::pollfd {
                fd: k.device.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: polls is live initialized pollfd storage; poll receives its exact length.
        let ready = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 50) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("keyboard poll failed");
        }
        let mut removed = Vec::new();
        for ((path, keyboard), poll) in devices.iter_mut().zip(polls.iter()) {
            if poll.revents == 0 {
                continue;
            }
            if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                removed.push(path.clone());
                continue;
            }
            let events = match keyboard.device.fetch_events() {
                Ok(events) => events.collect::<Vec<_>>(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(error) if error.raw_os_error() == Some(libc::ENODEV) => {
                    removed.push(path.clone());
                    continue;
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed reading keyboard {}", path.display()))
                }
            };
            if events.is_empty() {
                removed.push(path.clone());
                continue;
            }
            let now_ns = monotonic_now()?;
            let utc = chrono::Utc::now();
            for event in events {
                if let EventSummary::Synchronization(_, code, _) = event.destructure() {
                    if code == SynchronizationCode::SYN_DROPPED {
                        keyboard.dropped = true;
                    } else if code == SynchronizationCode::SYN_REPORT && keyboard.dropped {
                        send(
                            &tx,
                            CaptureMessage::Reset {
                                source: keyboard.source.clone(),
                                held_keys: held(&keyboard.device)?,
                            },
                        )?;
                        keyboard.dropped = false;
                    }
                    continue;
                }
                if keyboard.dropped {
                    continue;
                }
                if let EventSummary::Key(_, code, value) = event.destructure() {
                    if !is_keyboard_key(code) {
                        continue;
                    }
                    let Some(action) = action(value) else {
                        continue;
                    };
                    let monotonic_ns =
                        u64::try_from(event.timestamp().duration_since(UNIX_EPOCH)?.as_nanos())?;
                    let age = now_ns.saturating_sub(monotonic_ns).min(i64::MAX as u64) as i64;
                    send(
                        &tx,
                        CaptureMessage::Event(KeyEvent {
                            source: keyboard.source.clone(),
                            key: key(code),
                            action,
                            monotonic_ns,
                            timestamp: utc - chrono::Duration::nanoseconds(age),
                        }),
                    )?;
                }
            }
        }
        for path in removed {
            if let Some(keyboard) = devices.remove(&path) {
                send(
                    &tx,
                    CaptureMessage::Reset {
                        source: keyboard.source,
                        held_keys: vec![],
                    },
                )?;
                eprintln!("Keyboard disconnected: {}", path.display());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_repeat_is_distinct_from_press() {
        assert_eq!(action(0), Some(KeyAction::Release));
        assert_eq!(action(1), Some(KeyAction::Press));
        assert_eq!(action(2), Some(KeyAction::Repeat));
        assert_eq!(action(3), None);
    }
    #[test]
    fn mouse_and_gamepad_buttons_are_not_keyboard_keys() {
        assert!(is_keyboard_key(KeyCode::KEY_A));
        assert!(is_keyboard_key(KeyCode::KEY_VOLUMEUP));
        assert!(!is_keyboard_key(KeyCode::BTN_LEFT));
        assert!(!is_keyboard_key(KeyCode::BTN_DPAD_UP));
    }
}
