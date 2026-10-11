#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::run;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::run;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(any(target_os = "windows", test))]
mod windows_keys;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn run(
    _: std::sync::mpsc::SyncSender<crate::event::CaptureMessage>,
    _: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<()> {
    anyhow::bail!("keyboard capture is supported on Windows, macOS, and Linux")
}
