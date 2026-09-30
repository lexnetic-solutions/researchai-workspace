//! Phase 0 ingestion staging: validates selected files, computes checksums
//! for duplicate detection and returns the count accepted into the future
//! processing queue. Parsing/OCR/indexing arrive in Phase 1 (spec §14).

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::services::storage::Workspace;

/// Extensions accepted in Phase 0. Extended by the parser registry in Phase 1.
const SUPPORTED: &[&str] = &["pdf", "docx", "pptx", "xlsx", "html", "htm", "md", "txt", "epub", "png", "jpg", "jpeg", "wav", "mp3", "m4a", "mp4", "mov"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub struct StagedFile {
    pub original_path: std::path::PathBuf,
    pub checksum: String,
    pub size_bytes: u64,
}

/// Stage a batch: validate each file, hash it, and reject duplicates *within
/// the batch*. Cross-batch dedup needs the documents table (Phase 1).
pub fn stage_batch(paths: &[String]) -> AppResult<Vec<StagedFile>> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut staged = Vec::new();

    for raw in paths {
        let path = Path::new(raw);
        Workspace::validate_source(path)?;

        if !is_supported(path) {
            return Err(AppError::msg(format!(
                "Unsupported file type: {}",
                path.display()
            )));
        }

        let checksum = hash_file(path)?;
        if !seen.insert(checksum.clone()) {
            continue; // duplicate inside this batch — skip silently
        }

        let size_bytes = std::fs::metadata(path)?.len();
        staged.push(StagedFile {
            original_path: path.to_path_buf(),
            checksum,
            size_bytes,
        });
    }
    Ok(staged)
}

pub fn hash_file(path: &Path) -> AppResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_extension() {
        assert!(!is_supported(Path::new("/tmp/evil.exe")));
        assert!(!is_supported(Path::new("/tmp/noext")));
        assert!(is_supported(Path::new("/tmp/paper.pdf")));
        assert!(is_supported(Path::new("/tmp/NOTES.MD")));
    }

    #[test]
    fn hashes_stably() {
        let dir = std::env::temp_dir().join(format!("researchai-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"hello").unwrap();
        let h1 = hash_file(&f).unwrap();
        let h2 = hash_file(&f).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
