use crate::record::{LogRecord, RecordKind};
use anyhow::{Context, Result};
use fs2::FileExt;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub struct LogStore {
    file: File,
    path: PathBuf,
}
pub struct SavedRecord {
    pub record: LogRecord,
    pub json: String,
}

struct Lock<'a>(&'a File);
impl<'a> Lock<'a> {
    fn exclusive(file: &'a File) -> Result<Self> {
        FileExt::lock_exclusive(file)?;
        Ok(Self(file))
    }
    fn shared(file: &'a File) -> Result<Self> {
        FileExt::lock_shared(file)?;
        Ok(Self(file))
    }
}
impl Drop for Lock<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.0);
    }
}

fn validate_tail(file: &File) -> Result<()> {
    let mut reader = file;
    if file.metadata()?.len() > 0 {
        reader.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        reader.read_exact(&mut last)?;
        anyhow::ensure!(
            last[0] == b'\n',
            "log has an incomplete trailing record; inspect or clear it before monitoring"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn directory_sync_paths(parent: &Path) -> Result<Vec<PathBuf>> {
    let mut current = if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        std::env::current_dir()?.join(parent)
    };
    let mut paths = vec![current.clone()];
    // Collect before creating directories: each new directory has a directory
    // entry in its parent that must be synced as well as the log's own entry.
    while !current.exists() {
        current = current
            .parent()
            .context("log directory has no existing ancestor")?
            .to_path_buf();
        paths.push(current.clone());
    }
    Ok(paths)
}

impl LogStore {
    pub fn open(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        #[cfg(unix)]
        let sync_paths = directory_sync_paths(parent)?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(parent)
            .with_context(|| format!("cannot create log directory {}", parent.display()))?;
        let mut options = OpenOptions::new();
        options.read(true).append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .with_context(|| format!("cannot open log {}", path.display()))?;
        {
            let _lock = Lock::exclusive(&file)?;
            anyhow::ensure!(
                file.metadata()?.is_file(),
                "log path must be a regular file: {}",
                path.display()
            );
            validate_tail(&file).with_context(|| format!("cannot append to {}", path.display()))?;
            file.sync_all()?;
            #[cfg(unix)]
            for directory in sync_paths {
                File::open(&directory)?.sync_all().with_context(|| {
                    format!("cannot sync log directory {}", directory.display())
                })?;
            }
        }
        Ok(Self {
            file,
            path: path.into(),
        })
    }
    pub fn append(&self, record: &LogRecord) -> Result<()> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        let result = (|| -> Result<()> {
            let _lock = Lock::exclusive(&self.file)?;
            validate_tail(&self.file)?;
            let mut writer = &self.file;
            writer.write_all(&line)?;
            writer.flush()?;
            self.file.sync_all()?;
            Ok(())
        })();
        result.with_context(|| format!("failed to save record to {}", self.path.display()))
    }
}

pub fn view(path: &Path, kind: RecordKind, limit: usize) -> Result<Vec<SavedRecord>> {
    anyhow::ensure!(limit > 0, "limit must be positive");
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read log {}", path.display()))
        }
    };
    let _lock = Lock::shared(&file)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "log path must be a regular file"
    );
    let mut rows = VecDeque::new();
    for (i, line) in BufReader::new(&file).lines().enumerate() {
        let json =
            line.with_context(|| format!("cannot read {} at line {}", path.display(), i + 1))?;
        let record: LogRecord = serde_json::from_str(&json).with_context(|| {
            format!("invalid log record in {} at line {}", path.display(), i + 1)
        })?;
        anyhow::ensure!(
            record.press().schema_version == 1,
            "unsupported schema version {} at line {}",
            record.press().schema_version,
            i + 1
        );
        if kind == RecordKind::All || record.kind() == kind {
            if rows.len() == limit {
                rows.pop_front();
            }
            rows.push_back(SavedRecord { record, json });
        }
    }
    Ok(rows.into_iter().collect())
}

pub fn clear(path: &Path, confirmed: bool) -> Result<bool> {
    anyhow::ensure!(confirmed, "clearing {} requires --yes; run logs clear --yes with the same --output to remove its records",path.display());
    let file = match OpenOptions::new().write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot clear log {}", path.display()))
        }
    };
    let _lock = Lock::exclusive(&file)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "log path must be a regular file"
    );
    let had_records = file.metadata()?.len() > 0;
    file.set_len(0)
        .with_context(|| format!("cannot clear {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("cannot sync cleared log {}", path.display()))?;
    Ok(had_records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{Key, KeyAction, KeyEvent, Source},
        record::PressRecord,
    };

    #[test]
    fn actual_write_failure_is_reported_and_existing_contents_remain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        std::fs::write(&path, b"existing\n").unwrap();
        let store = LogStore {
            file: File::open(&path).unwrap(),
            path: path.clone(),
        };
        let event = KeyEvent {
            source: Source {
                id: "one".into(),
                name: None,
            },
            key: Key {
                code: 30,
                name: "A".into(),
            },
            action: KeyAction::Press,
            monotonic_ns: 0,
            timestamp: chrono::Utc::now(),
        };
        let error = store
            .append(&LogRecord::Keystroke(PressRecord::new(&event, "test")))
            .unwrap_err();
        assert!(error.to_string().contains("failed to save record"));
        assert_eq!(std::fs::read(&path).unwrap(), b"existing\n");
    }

    #[cfg(unix)]
    #[test]
    fn creating_nested_log_directories_syncs_their_ancestor_entries() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let nested = first.join("second");
        assert_eq!(
            directory_sync_paths(&nested).unwrap(),
            vec![nested.clone(), first.clone(), dir.path().to_path_buf()]
        );
        std::fs::create_dir(&first).unwrap();
        assert_eq!(directory_sync_paths(&nested).unwrap(), vec![nested, first]);
    }
}
