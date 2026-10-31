use crate::{
    capture,
    detector::Range,
    display::{MonitorDisplay, TerminalFormat},
    event::CaptureMessage,
    record::RecordKind,
    session::Session,
    storage::{self, LogStore},
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::{
    io::{self, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
};

#[derive(Parser)]
#[command(
    version,
    about = "Detect physical keyboard chatter and manage persistent keyboard logs"
)]
pub struct Cli {
    /// JSONL file for monitoring, viewing, and clearing (default: local user data directory)
    #[arg(long, global = true)]
    output: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Monitor physical keyboard presses across applications
    Monitor(Monitor),
    /// View or clear previously saved records without keyboard capture
    Logs {
        #[command(subcommand)]
        command: Logs,
    },
}

#[derive(Args)]
struct Monitor {
    /// Minimum press-to-press interval in milliseconds (inclusive)
    #[arg(long, default_value_t = 0.0)]
    min_ms: f64,
    /// Maximum press-to-press interval in milliseconds (inclusive)
    #[arg(long, default_value_t = 80.0)]
    max_ms: f64,
    /// Also save every distinct physical key press; held-key repeats remain excluded
    #[arg(long)]
    all_keys: bool,
}

#[derive(Subcommand)]
enum Logs {
    /// Display the latest matching records, in recorded order
    View {
        #[arg(long,value_enum,default_value_t=RecordKind::All)]
        kind: RecordKind,
        #[arg(long, default_value = "100")]
        limit: NonZeroUsize,
        /// Print the original JSONL records for use in scripts
        #[arg(long)]
        json: bool,
    },
    /// Permanently remove all records from the selected file
    Clear {
        /// Confirm clearing the selected log file
        #[arg(long)]
        yes: bool,
    },
}

fn output_path(selected: Option<PathBuf>) -> Result<PathBuf> {
    let path = match selected {
        Some(path) => path,
        None => dirs::data_local_dir()
            .context("cannot locate local user data directory; specify --output")?
            .join("bg-key-dbclk-checker")
            .join("symptoms.jsonl"),
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub fn run(cli: Cli) -> Result<()> {
    let path = output_path(cli.output)?;
    match cli.command {
        Command::Monitor(options) => monitor(path, options),
        Command::Logs {
            command: Logs::View { kind, limit, json },
        } => {
            let rows = storage::view(&path, kind, limit.get())?;
            let mut output = anstream::AutoStream::new(
                io::stdout().lock(),
                if json {
                    anstream::ColorChoice::Never
                } else {
                    anstream::ColorChoice::Auto
                },
            );
            let format = TerminalFormat::new(true);
            if json {
                for row in rows {
                    writeln!(output, "{}", row.json)?;
                }
            } else if rows.is_empty() {
                writeln!(output, "No matching records in {}", path.display())?;
            } else {
                format.write_logs(&rows, &mut output)?;
            }
            Ok(())
        }
        Command::Logs {
            command: Logs::Clear { yes },
        } => {
            let changed = storage::clear(&path, yes)?;
            writeln!(
                io::stdout().lock(),
                "{} {}",
                if changed {
                    "Cleared"
                } else {
                    "No records to clear in"
                },
                path.display()
            )?;
            Ok(())
        }
    }
}

fn monitor(path: PathBuf, options: Monitor) -> Result<()> {
    let range = Range::new(options.min_ms, options.max_ms)?;
    let store = LogStore::open(&path)?;
    let session_id = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
        std::process::id()
    );
    let session = Session::new(range, options.all_keys, store, session_id);
    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = stop.clone();
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::Relaxed))
        .context("cannot install Ctrl+C handler")?;
    let format = TerminalFormat::new(true);
    {
        let mut status =
            anstream::AutoStream::new(io::stderr().lock(), anstream::ColorChoice::Auto);
        format.write_monitor_status(&mut status, &path, range, options.all_keys)?;
    }
    let (tx, rx) = mpsc::sync_channel(4096);
    let capture_stop = stop.clone();
    let worker = thread::spawn(move || capture::run(tx, capture_stop));
    let mut output = anstream::AutoStream::new(io::stdout().lock(), anstream::ColorChoice::Auto);
    monitor_session(session, rx, worker, &stop, &mut output, &format)?;
    eprintln!(
        "Monitoring stopped; saved records remain in {}",
        path.display()
    );
    Ok(())
}

fn monitor_session(
    mut session: Session,
    rx: mpsc::Receiver<CaptureMessage>,
    worker: thread::JoinHandle<Result<()>>,
    stop: &AtomicBool,
    output: &mut impl Write,
    format: &TerminalFormat,
) -> Result<()> {
    let display = MonitorDisplay::for_terminal(format);
    let result = (|| -> Result<()> {
        display.write_header(output)?;
        // Drain timestamped events until capture closes, including events already
        // queued when Ctrl+C was received, so detected symptoms remain durable.
        while let Ok(message) = rx.recv() {
            if let Some(symptom) = session.handle(message)? {
                display.write_symptom(output, &symptom)?;
            }
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    drop(rx);
    let capture_result = worker
        .join()
        .map_err(|_| anyhow::anyhow!("keyboard capture thread panicked"));
    result?;
    capture_result??;
    session.write_summary_with_format(output, format)?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{CaptureMessage, Key, KeyAction, KeyEvent, Source},
        record::RecordKind,
    };

    #[test]
    fn ctrl_c_drains_queued_symptoms_before_printing_summary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let session = Session::new(
            Range::new(0.0, 80.0).unwrap(),
            false,
            LogStore::open(&path).unwrap(),
            "current".into(),
        );
        let (tx, rx) = mpsc::sync_channel(16);
        for (action, monotonic_ns) in [
            (KeyAction::Press, 100_000_000),
            (KeyAction::Release, 110_000_000),
            (KeyAction::Press, 130_750_000),
            (KeyAction::Release, 140_000_000),
            (KeyAction::Press, 160_500_000),
        ] {
            tx.send(CaptureMessage::Event(KeyEvent {
                source: Source {
                    id: "keyboard".into(),
                    name: Some("Mechanical Keyboard".into()),
                },
                key: Key {
                    code: 30,
                    name: "A".into(),
                },
                action,
                monotonic_ns,
                timestamp: chrono::Utc::now(),
            }))
            .unwrap();
        }
        let stop = AtomicBool::new(true); // Ctrl+C with events still queued.
        let worker = thread::spawn(move || {
            drop(tx);
            Ok(())
        });
        let mut output = Vec::new();
        monitor_session(
            session,
            rx,
            worker,
            &stop,
            &mut output,
            &TerminalFormat::new(false),
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        let (live, summary) = output.split_once("Double-click summary").unwrap();
        assert_eq!(live.matches("Time (local)").count(), 1, "{output}");
        let rows: Vec<Vec<&str>> = live
            .lines()
            .filter(|line| line.contains(" ms"))
            .map(|line| line.split('|').map(str::trim).collect())
            .collect();
        assert_eq!(rows.len(), 2, "{output}");
        for (row, delay) in rows.iter().zip(["31 ms", "30 ms"]) {
            assert_eq!(row.len(), 7);
            assert!(chrono::NaiveDateTime::parse_from_str(row[1], "%Y-%m-%d %H:%M:%S").is_ok());
            assert_eq!(
                &row[2..6],
                &["\"A\"", "30", delay, "\"Mechanical Keyboard\""]
            );
        }
        assert!(output.contains("Total symptoms: 2"));
        assert!(summary.contains("Count"));
        assert!(!output.contains("key="));
        assert!(!output.contains("interval="));
        assert_eq!(
            storage::view(&path, RecordKind::Symptom, 100)
                .unwrap()
                .len(),
            2
        );
    }
}
