//! Crash-safe filesystem intents for configuration recovery.
//!
//! These markers live outside SQLite deliberately. The database-replacement
//! marker must exist before a corrupt database is moved, including across a
//! crash between the final rename and creation of the replacement database.
//! The panel marker carries a one-shot UI intent across Tauri's process
//! relaunch without changing the saved configuration that Retry inspects.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

use tomari_core::AppPaths;

pub(crate) fn database_reset_required(paths: &AppPaths) -> std::io::Result<bool> {
    marker_present(&paths.database_reset_required_path)
}

pub(crate) fn arm_database_reset_required(paths: &AppPaths) -> std::io::Result<()> {
    arm_marker(&paths.database_reset_required_path)
}

pub(crate) fn clear_database_reset_required(paths: &AppPaths) -> std::io::Result<()> {
    clear_marker(&paths.database_reset_required_path)
}

pub(crate) fn show_panel_after_recovery(paths: &AppPaths) -> std::io::Result<bool> {
    marker_present(&paths.show_panel_after_recovery_path)
}

pub(crate) fn arm_show_panel_after_recovery(paths: &AppPaths) -> std::io::Result<()> {
    arm_marker(&paths.show_panel_after_recovery_path)
}

pub(crate) fn clear_show_panel_after_recovery(paths: &AppPaths) -> std::io::Result<()> {
    clear_marker(&paths.show_panel_after_recovery_path)
}

/// Treat any directory entry as an armed marker. `Path::exists` turns metadata
/// errors and dangling symlinks into `false`, which could silently remove a
/// safety interlock.
fn marker_present(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Create and flush a write-ahead marker before the guarded operation starts.
///
/// An existing entry is success: the intent is already armed, even if an
/// earlier crash left something other than the ordinary marker file at that
/// path. Clearing remains strict and will refuse to remove a directory.
fn arm_marker(path: &Path) -> std::io::Result<()> {
    if marker_present(path)? {
        return sync_parent(path);
    }

    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return sync_parent(path),
        Err(error) => return Err(error),
    };
    file.write_all(b"1\n")?;
    file.sync_all()?;
    sync_parent(path)
}

/// Remove a consumed marker and flush the directory entry change. Failure is
/// returned to the caller so a recovery command never relaunches while its
/// durable interlock may still be present.
fn clear_marker(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sync_parent(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(ErrorKind::InvalidInput, "recovery marker has no parent")
    })?;
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_durable_idempotent_and_independent() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(directory.path());

        assert!(!database_reset_required(&paths).unwrap());
        assert!(!show_panel_after_recovery(&paths).unwrap());

        arm_database_reset_required(&paths).unwrap();
        arm_database_reset_required(&paths).unwrap();
        assert!(database_reset_required(&paths).unwrap());
        assert!(!show_panel_after_recovery(&paths).unwrap());
        assert_eq!(
            std::fs::read(&paths.database_reset_required_path).unwrap(),
            b"1\n"
        );

        arm_show_panel_after_recovery(&paths).unwrap();
        assert!(show_panel_after_recovery(&paths).unwrap());
        clear_database_reset_required(&paths).unwrap();
        clear_database_reset_required(&paths).unwrap();
        assert!(!database_reset_required(&paths).unwrap());
        assert!(show_panel_after_recovery(&paths).unwrap());
    }

    #[test]
    fn a_non_file_entry_stays_armed_when_clear_is_not_safe() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(directory.path());
        std::fs::create_dir(&paths.database_reset_required_path).unwrap();

        assert!(database_reset_required(&paths).unwrap());
        assert!(arm_database_reset_required(&paths).is_ok());
        assert!(clear_database_reset_required(&paths).is_err());
        assert!(database_reset_required(&paths).unwrap());
    }

    #[test]
    fn arm_fails_without_a_writable_parent_and_creates_nothing_else() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(directory.path().join("missing"));

        assert!(arm_database_reset_required(&paths).is_err());
        assert!(!directory.path().join("database-reset-required").exists());
    }
}
