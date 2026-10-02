//! Durable publication of owned database generations, independent of SQLite health.

use super::UsageResult;
use crate::preferences::TrackingDays;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MANIFEST: &str = "usage-active.json";
const MARKER: &str = "usage-rebuild.json";
const INITIAL_DATABASE: &str = "usage.sqlite3";

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    active: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rebuild {
    pub previous: String,
    pub fresh: String,
    pub days: TrackingDays,
    #[serde(default)]
    pub retired: Option<String>,
}

pub struct Generations {
    directory: PathBuf,
    pub active: String,
    pub pending: Option<Rebuild>,
}

impl Generations {
    pub fn load(directory: &Path) -> UsageResult<Self> {
        let manifest: Option<Manifest> = read_json(&directory.join(MANIFEST))?;
        let active = manifest.map_or_else(|| INITIAL_DATABASE.into(), |manifest| manifest.active);
        validate_name(&active)?;
        let pending: Option<Rebuild> = read_json(&directory.join(MARKER))?;
        if let Some(pending) = &pending {
            validate_name(&pending.previous)?;
            validate_name(&pending.fresh)?;
            if pending.fresh == pending.previous {
                return Err("rebuild generations must be distinct".into());
            }
            if let Some(retired) = &pending.retired {
                validate_name(retired)?;
                if retired == &pending.previous || retired == &pending.fresh {
                    return Err("invalid retired rebuild generation".into());
                }
            }
        }
        Ok(Self {
            directory: directory.to_path_buf(),
            active,
            pending,
        })
    }

    pub fn path(&self) -> PathBuf {
        self.directory.join(&self.active)
    }

    pub fn import_path(&self) -> PathBuf {
        self.directory.join(
            self.pending
                .as_ref()
                .map_or(self.active.as_str(), |pending| &pending.fresh),
        )
    }

    /// Called only while holding the service's exclusive writer lock.
    pub fn begin_rebuild(&mut self, days: TrackingDays) -> UsageResult<()> {
        self.finish_published_cleanup()?;
        let pending = self.pending.clone().map_or_else(
            || Rebuild {
                previous: self.active.clone(),
                fresh: format!(
                    "usage-{}-{}.sqlite3",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                    std::process::id()
                ),
                days,
                retired: None,
            },
            |pending| Rebuild { days, ..pending },
        );
        write_json(&self.directory.join(MARKER), &pending)?;
        #[cfg(test)]
        super::crash_tests::crash_at("intent");
        self.pending = Some(pending);
        Ok(())
    }

    pub fn ensure_manifest(&self) -> UsageResult<()> {
        if !self.directory.join(MANIFEST).exists() {
            write_json(
                &self.directory.join(MANIFEST),
                &Manifest {
                    active: self.active.clone(),
                },
            )?;
        }
        Ok(())
    }

    /// An explicit retry owns both generation names durably before cleanup.
    pub fn replace_pending(&mut self, days: TrackingDays) -> UsageResult<()> {
        self.cleanup_retired()?;
        let previous = self.pending.as_ref().ok_or("no rebuild is pending")?;
        let replacement = Rebuild {
            previous: previous.previous.clone(),
            fresh: format!(
                "usage-{}-{}.sqlite3",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                std::process::id()
            ),
            days,
            retired: Some(previous.fresh.clone()),
        };
        write_json(&self.directory.join(MARKER), &replacement)?;
        #[cfg(test)]
        super::crash_tests::crash_at("retired_intent");
        self.pending = Some(replacement);
        self.cleanup_retired()
    }

    /// The caller has validated, checkpointed and closed the fresh database.
    pub fn publish(&mut self) -> UsageResult<()> {
        let pending = self.pending.as_ref().ok_or("no rebuild is pending")?;
        write_json(
            &self.directory.join(MANIFEST),
            &Manifest {
                active: pending.fresh.clone(),
            },
        )?;
        #[cfg(test)]
        super::crash_tests::crash_at("published");
        self.active = pending.fresh.clone();
        self.finish_published_cleanup()
    }

    pub fn finish_published_cleanup(&mut self) -> UsageResult<()> {
        self.cleanup_retired()?;
        let Some(pending) = self
            .pending
            .as_ref()
            .filter(|pending| self.active == pending.fresh)
        else {
            return Ok(());
        };
        remove_database(&self.directory, &pending.previous)?;
        #[cfg(test)]
        super::crash_tests::crash_at("old_removed");
        remove_owned_file(&self.directory.join(MARKER))?;
        #[cfg(test)]
        super::crash_tests::crash_at("marker_removed");
        sync_directory(&self.directory)?;
        self.pending = None;
        Ok(())
    }

    fn cleanup_retired(&mut self) -> UsageResult<()> {
        let Some(retired) = self
            .pending
            .as_ref()
            .and_then(|pending| pending.retired.clone())
        else {
            return Ok(());
        };
        remove_database(&self.directory, &retired)?;
        #[cfg(test)]
        super::crash_tests::crash_at("retired_removed");
        let mut pending = self.pending.clone().ok_or("rebuild marker disappeared")?;
        pending.retired = None;
        write_json(&self.directory.join(MARKER), &pending)?;
        self.pending = Some(pending);
        Ok(())
    }
}

fn validate_name(name: &str) -> UsageResult<()> {
    if name == INITIAL_DATABASE {
        return Ok(());
    }
    let valid = name
        .strip_prefix("usage-")
        .and_then(|name| name.strip_suffix(".sqlite3"))
        .is_some_and(|id| {
            !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-')
        });
    if valid {
        return Ok(());
    }
    Err("invalid database generation in usage manifest".into())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> UsageResult<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> UsageResult<()> {
    let directory = path.parent().ok_or("manifest has no parent directory")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    #[cfg(test)]
    super::crash_tests::crash_at(if path.file_name().is_some_and(|name| name == MARKER) {
        "marker_synced"
    } else {
        "manifest_synced"
    });
    drop(file);
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    sync_directory(directory)
}

fn remove_database(directory: &Path, name: &str) -> UsageResult<()> {
    validate_name(name)?;
    for suffix in ["", "-wal", "-shm"] {
        remove_owned_file(&directory.join(format!("{name}{suffix}")))?;
    }
    sync_directory(directory)
}

fn remove_owned_file(path: &Path) -> UsageResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn sync_directory(directory: &Path) -> UsageResult<()> {
    #[cfg(unix)]
    {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())
    }
    #[cfg(not(unix))]
    {
        // File::sync_all plus the platform's atomic rename supplies publication;
        // std does not expose directory synchronization on Windows.
        let _ = directory;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "skillshard-generation-{tag}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn intent_resumes_before_publication_and_cleanup_resumes_after_it() {
        let directory = directory("recovery");
        let mut generations = Generations::load(&directory).unwrap();
        generations.begin_rebuild(TrackingDays::Seven).unwrap();
        let pending = generations.pending.clone().unwrap();
        fs::write(directory.join(&pending.previous), b"old").unwrap();
        fs::write(directory.join(&pending.fresh), b"fresh").unwrap();
        let recovered = Generations::load(&directory).unwrap();
        assert_eq!(recovered.active, pending.previous);
        assert_eq!(recovered.pending.unwrap().fresh, pending.fresh);

        // Simulate termination after publication but before cleanup.
        write_json(
            &directory.join(MANIFEST),
            &Manifest {
                active: pending.fresh.clone(),
            },
        )
        .unwrap();
        let mut recovered = Generations::load(&directory).unwrap();
        recovered.finish_published_cleanup().unwrap();
        assert_eq!(recovered.path(), directory.join(&pending.fresh));
        assert!(recovered.path().exists());
        assert!(!directory.join(&pending.previous).exists());
        assert!(!directory.join(MARKER).exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn window_changes_reuse_pending_generation_and_reject_unowned_paths() {
        let directory = directory("window");
        let mut generations = Generations::load(&directory).unwrap();
        generations.begin_rebuild(TrackingDays::Seven).unwrap();
        let fresh = generations.pending.as_ref().unwrap().fresh.clone();
        generations.begin_rebuild(TrackingDays::Sixty).unwrap();
        assert_eq!(generations.pending.as_ref().unwrap().fresh, fresh);
        assert_eq!(
            generations.pending.as_ref().unwrap().days,
            TrackingDays::Sixty
        );
        for name in [
            "../victim.sqlite3",
            "/tmp/victim",
            "skills.sqlite3",
            "usage-../../.sqlite3",
        ] {
            assert!(validate_name(name).is_err());
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
