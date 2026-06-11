//! Filesystem locations Tomari uses for its on-disk state.

use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::{Error, Result};

/// Resolved application directories.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
}

impl AppPaths {
    /// Resolve the standard per-user data directory for Tomari.
    pub fn resolve() -> Result<Self> {
        let proj = ProjectDirs::from("app", "Tomari", "Tomari").ok_or(Error::NoDataDir)?;
        Ok(Self::with_root(proj.data_dir()))
    }

    /// Build paths rooted at an arbitrary directory (used by tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let data_dir = root.into();
        let db_path = data_dir.join("tomari.sqlite");
        Self { data_dir, db_path }
    }

    /// Create the data directory if it does not already exist.
    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        Ok(())
    }
}
