//! Phase 0 ingestion staging: validates selected files, computes checksums
//! for duplicate detection and returns the count accepted into the future
//! processing queue. Parsing/OCR/indexing arrive in Phase 1 (spec §14).

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::services::storage::Workspace;

/// Extensions accepted for document import. This list is the contract with
/// the document engine's parser registry (`parsers/__init__.py`): every entry
/// here must have a real parser, otherwise a file imports cleanly and then
/// fails at parse time. Audio/video/images are deliberately absent — they are
/// not documents, they are handled by the Audio (transcription) flow.
pub const SUPPORTED: &[&str] = &[
    "pdf", "docx", "pptx", "xlsx", "html", "htm", "md", "txt", "epub",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Folder selections deeper than this are not walked further (defence against
/// pathological or self-referencing trees).
const MAX_WALK_DEPTH: usize = 12;

/// Human-readable allow-list for error messages and the native picker filter.
pub fn supported_list() -> String {
    SUPPORTED
        .iter()
        .map(|e| e.to_uppercase())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Result of expanding a picker selection: concrete files to import plus the
/// number of unsupported files found *inside* selected folders.
pub struct ExpandedSelection {
    pub files: Vec<std::path::PathBuf>,
    pub skipped_unsupported: usize,
}

/// Expand the native picker's output into a flat file list.
///
/// A picked *file* is passed through untouched (even if unsupported, so the
/// importer can report it to the user by name). A picked *directory* is walked
/// recursively for supported documents only — a real-world folder is full of
/// images, `.DS_Store` and other noise that must not fail the import, so those
/// are counted as skipped instead.
///
/// Directory symlinks are never followed (a `read_dir` entry reports symlink
/// type without resolving), which rules out walk cycles outright; the depth
/// cap guards the explicitly-picked top-level path.
pub fn expand_selection(paths: &[String]) -> ExpandedSelection {
    let mut files = Vec::new();
    let mut skipped_unsupported = 0usize;

    for raw in paths {
        let path = Path::new(raw);
        let is_dir = std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            walk_dir(path, 0, &mut files, &mut skipped_unsupported);
        } else {
            files.push(path.to_path_buf());
        }
    }

    ExpandedSelection {
        files,
        skipped_unsupported,
    }
}

fn walk_dir(
    dir: &Path,
    depth: usize,
    out: &mut Vec<std::path::PathBuf>,
    skipped: &mut usize,
) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return, // unreadable subfolder: skip, the rest still imports
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // hidden entries: .DS_Store, .git, .Spotlight-V100, …
        }
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            walk_dir(&path, depth + 1, out, skipped);
        } else if file_type.is_file() {
            if is_supported(&path) {
                out.push(path);
            } else {
                *skipped += 1;
            }
        }
        // Symlinks are neither dir nor file here (file_type does not follow),
        // so they are ignored rather than risked as a cycle or a broken link.
    }
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
        // The allow-list is the engine's parser registry: media files are not
        // documents and must not be offered by the picker.
        assert!(!is_supported(Path::new("/tmp/lecture.wav")));
        assert!(!is_supported(Path::new("/tmp/figure.png")));
    }

    #[test]
    fn supported_list_names_every_extension() {
        let listed = supported_list();
        for ext in SUPPORTED {
            assert!(listed.contains(&ext.to_uppercase()), "{ext} missing from {listed}");
        }
    }

    /// Fixture: a folder tree with supported docs, unsupported noise, a
    /// hidden file and a nested subfolder.
    fn fixture_tree() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "researchai-expand-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
        std::fs::write(root.join("top.md"), b"# top").unwrap();
        std::fs::write(root.join("noise.jpg"), b"jpeg").unwrap();
        std::fs::write(root.join(".DS_Store"), b"junk").unwrap();
        std::fs::write(root.join("nested/inner.pdf"), b"%PDF").unwrap();
        std::fs::write(root.join("nested/deeper/leaf.txt"), b"leaf").unwrap();
        std::fs::write(root.join("nested/deeper/song.mp3"), b"mp3").unwrap();
        root
    }

    #[test]
    fn expand_selection_walks_folders_for_supported_documents_only() {
        let root = fixture_tree();
        let raw = root.to_string_lossy().to_string();
        let selection = expand_selection(std::slice::from_ref(&raw));

        let mut names: Vec<String> = selection
            .files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["inner.pdf", "leaf.txt", "top.md"]);
        assert_eq!(selection.skipped_unsupported, 2); // noise.jpg + song.mp3

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expand_selection_passes_picked_files_through_untouched() {
        // Even an unsupported file the user picked explicitly must survive
        // expansion so the importer can name it in the error.
        let raw = "/tmp/definitely-not-supported.xyz".to_string();
        let selection = expand_selection(std::slice::from_ref(&raw));
        assert_eq!(selection.files.len(), 1);
        assert_eq!(selection.skipped_unsupported, 0);

        // A mixed batch: one file plus one folder.
        let root = fixture_tree();
        let batch = ["/tmp/missing.pdf".to_string(), root.to_string_lossy().to_string()];
        let selection = expand_selection(&batch);
        assert_eq!(selection.files.len(), 4); // missing.pdf + 3 found in the tree
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expand_selection_handles_absent_paths_without_panicking() {
        let raw = "/tmp/researchai-no-such-path-xyz".to_string();
        let selection = expand_selection(std::slice::from_ref(&raw));
        // Absent paths are passed through to import_file, which reports
        // "File not found" per file instead of aborting the batch.
        assert_eq!(selection.files.len(), 1);
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
