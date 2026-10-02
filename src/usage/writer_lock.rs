//! Explicit ownership releases locks even across inherited file descriptors.

use crate::usage::UsageResult;
use std::fs::{self, File, OpenOptions};
use std::path::Path;

pub(in crate::usage::service) struct WriterLock(File);

impl Drop for WriterLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            eprintln!("could not release the local usage writer lock: {error}");
        }
    }
}

pub(in crate::usage::service) fn acquire_lock(directory: &Path) -> UsageResult<WriterLock> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("usage-writer.lock"))
        .map_err(|error| error.to_string())?;
    file.try_lock().map_err(|error| format!("Local usage tracking is already owned by another app instance, or its writer lock is unavailable: {error}"))?;
    Ok(WriterLock(file))
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn writer_owner_releases_its_lock_while_an_inherited_descriptor_is_alive() {
        let directory = std::env::temp_dir().join(format!(
            "skillshard-lock-inheritance-{}",
            std::process::id()
        ));
        let owner = acquire_lock(&directory).unwrap();
        let inherited = owner.0.try_clone().unwrap();
        drop(owner);
        let next = acquire_lock(&directory).unwrap();
        drop(next);
        drop(inherited);
    }
}
