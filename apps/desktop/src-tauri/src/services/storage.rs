//! Managed workspace layout (spec §13). All paths are derived from the
//! resolved data directory — no OS-specific logic beyond `std::path`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn new(data_dir: &Path) -> AppResult<Self> {
        let root = data_dir.to_path_buf();
        for sub in ["documents", "derived", "audio", "exports", "models", "thumbnails", "cache"] {
            fs::create_dir_all(root.join(sub))?;
        }
        Ok(Self { root })
    }

    pub fn documents_dir(&self) -> PathBuf {
        self.root.join("documents")
    }

    pub fn exports_dir(&self) -> PathBuf {
        self.root.join("exports")
    }

    /// Destination for a managed copy: `documents/<project_id>/<checksum><ext>`.
    pub fn managed_document_path(&self, project_id: &str, checksum: &str, ext: &str) -> PathBuf {
        self.documents_dir().join(project_id).join(format!("{checksum}.{ext}"))
    }

    /// Validate that a path exists, is a file and is inside the user's
    /// readable filesystem. Path-traversal is irrelevant here because we
    /// never join user input with a base directory — we read what the OS
    /// file picker returned.
    pub fn validate_source(path: &Path) -> AppResult<()> {
        if !path.exists() {
            return Err(AppError::msg(format!(
                "File not found: {}",
                path.display()
            )));
        }
        if !path.is_file() {
            return Err(AppError::msg(format!(
                "Not a regular file: {}",
                path.display()
            )));
        }
        Ok(())
    }
}
