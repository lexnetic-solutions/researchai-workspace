//! Local model manager (spec §17).
//!
//! Owns everything GGUF: validation (magic + header metadata), the managed
//! `models/` directory inside the data dir, in-place imports, streamed URL
//! downloads with progress/cancel, and registry lifecycle. Nothing outside
//! this module touches model files.
//!
//! Integrity: SHA-256 is computed while copying/downloading (one pass) and
//! stored in `local_models.sha256`; `scan_for_missing` keeps statuses honest
//! when users move or delete files behind the app's back.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sha2::Digest;

use crate::db::{Db, LocalModelRow};
use crate::error::{AppError, AppResult};

/// GGUF files start with the magic bytes "GGUF".
const GGUF_MAGIC: [u8; 4] = [0x47, 0x47, 0x55, 0x46];

/// GGUF metadata value types (spec: ggml-p/gguf).
const GGUF_TYPE_UINT32: u32 = 4;
const GGUF_TYPE_INT32: u32 = 5;
const GGUF_TYPE_STRING: u32 = 8;
const GGUF_TYPE_UINT64: u32 = 10;
const GGUF_TYPE_INT64: u32 = 11;

/// Copy buffer: 1 MiB keeps memory flat on 8 GB machines.
const COPY_BUF: usize = 1024 * 1024;

pub struct ModelManager {
    models_dir: PathBuf,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GgufMeta {
    pub parameters: Option<String>,
    pub quantization: Option<String>,
    /// Parsed from the header when present; else None (user-tunable later).
    pub context_tokens: Option<i64>,
}

impl ModelManager {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            models_dir: data_dir.join("models"),
        }
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    fn ensure_dir(&self) -> AppResult<()> {
        std::fs::create_dir_all(&self.models_dir)
            .map_err(|e| AppError::msg(format!("Could not create models directory: {e}")))
    }

    /// Quick header sniff: is this file plausibly GGUF?
    pub fn is_gguf(path: &Path) -> bool {
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic).is_ok() && magic == GGUF_MAGIC
    }

    /// Parse the interesting keys from the GGUF header (general.* metadata).
    /// Stops at the tensor-info section; we never read model weights.
    pub fn read_gguf_meta(path: &Path) -> AppResult<GgufMeta> {
        let mut f = std::fs::File::open(path)
            .map_err(|e| AppError::msg(format!("Cannot open GGUF file: {e}")))?;
        if read_u32le(&mut f)? != u32::from_le_bytes(GGUF_MAGIC) {
            return Err(AppError::msg("Not a GGUF file (bad magic)."));
        }
        let _version = read_u32le(&mut f)?;
        let _tensor_count = read_u64le(&mut f)?;
        let kv_count = read_u64le(&mut f)?;

        let mut meta = GgufMeta::default();
        for _ in 0..kv_count.min(1024) {
            let key_len = read_u64le(&mut f)? as usize;
            if key_len > 4096 {
                return Err(AppError::msg("GGUF header is corrupt (key too long)."));
            }
            let key = String::from_utf8_lossy(&read_exact_n(&mut f, key_len)?).to_string();
            let vtype = read_u32le(&mut f)?;

            match (key.as_str(), vtype) {
                ("general.parameter_count", GGUF_TYPE_UINT64) => {
                    let v = read_u64le(&mut f)?;
                    meta.parameters = Some(format_parameter_count(v));
                }
                ("general.file_type", GGUF_TYPE_UINT32) => {
                    let v = read_u32le(&mut f)?;
                    meta.quantization = quant_from_file_type(v).map(str::to_string);
                }
                (_, GGUF_TYPE_UINT64 | GGUF_TYPE_INT64) => {
                    let _ = read_u64le(&mut f)?;
                }
                (_, GGUF_TYPE_UINT32 | GGUF_TYPE_INT32) => {
                    let _ = read_u32le(&mut f)?;
                }
                (_, GGUF_TYPE_STRING) => {
                    let len = read_u64le(&mut f)? as usize;
                    if len > 1 << 20 {
                        return Err(AppError::msg("GGUF header is corrupt (string too long)."));
                    }
                    let _ = read_exact_n(&mut f, len)?;
                }
                // Unknown type: the header walk cannot continue safely.
                _ => break,
            }
        }
        Ok(meta)
    }

    /// Import an existing GGUF file by copying it into the managed `models/`
    /// directory (originals are never moved). Registers it with the DB.
    pub fn import_from_path(
        &self,
        db: &Db,
        source: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> AppResult<LocalModelRow> {
        self.ensure_dir()?;
        if !source.is_file() {
            return Err(AppError::msg("The selected file does not exist."));
        }
        if !Self::is_gguf(source) {
            return Err(AppError::msg(
                "Not a GGUF model file — local models must be .gguf (llama.cpp format).",
            ));
        }

        let file_name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "model.gguf".to_string());
        let dest = self.models_dir.join(&file_name);
        if dest.exists() {
            return Err(AppError::msg(format!(
                "A model named \"{file_name}\" is already in the library."
            )));
        }

        let mut hasher = sha2::Sha256::new();
        let mut size: i64 = 0;
        let mut src = std::fs::File::open(source)?;
        let mut out = std::fs::File::create(&dest)?;
        let mut buf = vec![0u8; COPY_BUF];
        loop {
            if cancel.load(Ordering::SeqCst) {
                drop(out);
                let _ = std::fs::remove_file(&dest);
                return Err(AppError::msg("Import cancelled."));
            }
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            std::io::Write::write_all(&mut out, &buf[..n])?;
            size += n as i64;
        }

        let meta = Self::read_gguf_meta(&dest).unwrap_or_default();
        let row = LocalModelRow {
            id: uuid::Uuid::new_v4().to_string(),
            file_name,
            file_path: dest.to_string_lossy().to_string(),
            size_bytes: size,
            sha256: hex::encode(hasher.finalize()),
            parameters: meta.parameters,
            quantization: meta.quantization,
            context_tokens: meta.context_tokens,
            status: "available".into(),
            status_detail: None,
            source: "imported".into(),
            added_at: crate::db::now_iso_pub(),
            last_used_at: None,
        };
        db.insert_local_model(&row)?;
        Ok(row)
    }

    /// Download a GGUF from `url` into the managed directory with progress
    /// callbacks and cooperative cancellation. Loopback-only rule (spec §40)
    /// forbids non-local URLs — downloads are explicit user actions, so
    /// http(s) URLs are allowed, but the client pins no proxy and sends no
    /// identifying headers.
    pub fn download(
        &self,
        db: &Db,
        url: &str,
        cancel: &Arc<AtomicBool>,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> AppResult<LocalModelRow> {
        self.ensure_dir()?;
        let url = url.trim();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(AppError::msg("Model URL must start with http:// or https://."));
        }

        let file_name = url
            .split(['?', '#'])
            .next()
            .and_then(|p| p.rsplit('/').next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "model.gguf".to_string());
        let dest = self.models_dir.join(&file_name);
        if dest.exists() {
            return Err(AppError::msg(format!(
                "A model named \"{file_name}\" is already in the library."
            )));
        }

        let agent = {
            let config = ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(3600)))
                .build();
            ureq::Agent::new_with_config(config)
        };
        let resp = agent
            .get(url)
            .call()
            .map_err(|e| AppError::msg(format!("Download failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::msg(format!(
                "Download failed: server returned HTTP {}.",
                resp.status()
            )));
        }

        let total: u64 = resp
            .headers()
            .get(ureq::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let mut hasher = sha2::Sha256::new();
        let mut size: u64 = 0;
        let mut out = std::fs::File::create(&dest)?;
        let mut reader = resp.into_body().into_reader();
        let mut buf = vec![0u8; COPY_BUF];
        loop {
            if cancel.load(Ordering::SeqCst) {
                drop(out);
                let _ = std::fs::remove_file(&dest);
                return Err(AppError::msg("Download cancelled."));
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            std::io::Write::write_all(&mut out, &buf[..n])?;
            size += n as u64;
            if total > 0 && size % (32 * 1024 * 1024) < COPY_BUF as u64 {
                on_progress(size, total);
            }
        }
        on_progress(size, if total > 0 { total } else { size });

        if size < GGUF_MAGIC.len() as u64 {
            let _ = std::fs::remove_file(&dest);
            return Err(AppError::msg("Downloaded file is not a GGUF model."));
        }
        let mut head = [0u8; 4];
        {
            let mut f = std::fs::File::open(&dest)?;
            std::io::Read::read_exact(&mut f, &mut head)?;
        }
        if head != GGUF_MAGIC {
            let _ = std::fs::remove_file(&dest);
            return Err(AppError::msg(
                "Downloaded file is not a GGUF model (bad magic).",
            ));
        }

        let meta = Self::read_gguf_meta(&dest).unwrap_or_default();
        let row = LocalModelRow {
            id: uuid::Uuid::new_v4().to_string(),
            file_name,
            file_path: dest.to_string_lossy().to_string(),
            size_bytes: size as i64,
            sha256: hex::encode(hasher.finalize()),
            parameters: meta.parameters,
            quantization: meta.quantization,
            context_tokens: meta.context_tokens,
            status: "available".into(),
            status_detail: None,
            source: "downloaded".into(),
            added_at: crate::db::now_iso_pub(),
            last_used_at: None,
        };
        db.insert_local_model(&row)?;
        Ok(row)
    }

    /// Reconcile the registry with the filesystem: mark rows whose file
    /// vanished as `missing`, revive rows whose file returned. Returns the
    /// number of status changes.
    pub fn scan_for_missing(&self, db: &Db) -> AppResult<usize> {
        let mut changes = 0;
        for m in db.list_local_models()? {
            let exists = Path::new(&m.file_path).is_file();
            let on_disk = if exists { "available" } else { "missing" };
            if m.status != on_disk {
                db.update_local_model_status(&m.id, on_disk, None)?;
                changes += 1;
            }
        }
        Ok(changes)
    }

    /// Remove a model. `remove_file` also deletes the GGUF from disk.
    pub fn delete_model(&self, db: &Db, model_id: &str, remove_file: bool) -> AppResult<()> {
        let m = db.get_local_model(model_id)?;
        db.delete_local_model(model_id)?;
        if remove_file {
            let _ = std::fs::remove_file(&m.file_path);
        }
        Ok(())
    }
}

// -- helpers -----------------------------------------------------------------

fn read_exact_n(r: &mut impl Read, n: usize) -> AppResult<Vec<u8>> {
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)
        .map_err(|_| AppError::msg("GGUF header ended unexpectedly."))?;
    Ok(v)
}

fn read_u32le(r: &mut impl Read) -> AppResult<u32> {
    let b = read_exact_n(r, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64le(r: &mut impl Read) -> AppResult<u64> {
    let b = read_exact_n(r, 8)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

fn format_parameter_count(v: u64) -> String {
    if v >= 1_000_000_000 {
        format!("{:.1}B", v as f64 / 1e9)
    } else if v >= 1_000_000 {
        format!("{:.0}M", v as f64 / 1e6)
    } else {
        v.to_string()
    }
}

/// Common entries of GGUF's `general.file_type` enum.
fn quant_from_file_type(v: u32) -> Option<&'static str> {
    Some(match v {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        24 => "IQ3_XXS",
        30 => "IQ4_XS",
        _ => return None,
    })
}

// -- tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::{temp_dir_for, TempDir};
    use std::io::Write;

    fn db_and_manager(label: &str) -> (TempDir, Db, ModelManager) {
        let dir = temp_dir_for(label);
        let db = Db::open(dir.path()).unwrap();
        let mgr = ModelManager::new(dir.path());
        (dir, db, mgr)
    }

    /// A tiny file that starts with GGUF magic (enough for validation; the
    /// header walk in read_gguf_meta may fail on truncation, which is fine).
    fn fake_gguf(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let mut p = dir.to_path_buf();
        p.push(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(&GGUF_MAGIC).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn rejects_non_gguf_files() {
        let (_dir, db, mgr) = db_and_manager("mm-reject");
        let src = _dir.path().join("notes.txt");
        std::fs::write(&src, b"hello world, definitely not a model").unwrap();
        let err = mgr
            .import_from_path(&db, &src, &Arc::new(AtomicBool::new(false)))
            .unwrap_err();
        assert!(err.to_string().contains("GGUF"), "{err}");
    }

    #[test]
    fn import_registers_model_with_hash_and_dedups() {
        let (_dir, db, mgr) = db_and_manager("mm-import");
        let src = fake_gguf(_dir.path(), "tiny.gguf", &[0u8; 256]);
        let cancel = Arc::new(AtomicBool::new(false));
        let row = mgr.import_from_path(&db, &src, &cancel).unwrap();

        assert_eq!(row.file_name, "tiny.gguf");
        assert_eq!(row.status, "available");
        assert_eq!(row.source, "imported");
        assert_eq!(row.sha256.len(), 64);
        assert!(Path::new(&row.file_path).is_file());
        assert!(row.file_path.contains("models"));

        // Same source again → the destination name is taken.
        assert!(mgr.import_from_path(&db, &src, &cancel).is_err());
        assert_eq!(db.list_local_models().unwrap().len(), 1);

        // Re-running scan with the file present keeps it available.
        assert_eq!(mgr.scan_for_missing(&db).unwrap(), 0);
    }

    #[test]
    fn scan_flags_missing_then_revived_files() {
        let (_dir, db, mgr) = db_and_manager("mm-scan");
        let src = fake_gguf(_dir.path(), "gone.gguf", &[0u8; 32]);
        let row = mgr
            .import_from_path(&db, &src, &Arc::new(AtomicBool::new(false)))
            .unwrap();

        std::fs::remove_file(&row.file_path).unwrap();
        assert_eq!(mgr.scan_for_missing(&db).unwrap(), 1);
        assert_eq!(db.get_local_model(&row.id).unwrap().status, "missing");

        // Restore a file at the same path → scan revives it.
        std::fs::write(&row.file_path, b"whatever").unwrap();
        assert_eq!(mgr.scan_for_missing(&db).unwrap(), 1);
        assert_eq!(db.get_local_model(&row.id).unwrap().status, "available");
    }

    #[test]
    fn delete_model_removes_registry_and_optionally_the_file() {
        let (_dir, db, mgr) = db_and_manager("mm-delete");
        let src = fake_gguf(_dir.path(), "bye.gguf", &[0u8; 32]);
        let row = mgr
            .import_from_path(&db, &src, &Arc::new(AtomicBool::new(false)))
            .unwrap();

        mgr.delete_model(&db, &row.id, true).unwrap();
        assert!(db.get_local_model(&row.id).is_err());
        assert!(!Path::new(&row.file_path).exists());
        assert!(mgr.delete_model(&db, &row.id, false).is_err());
    }

    #[test]
    fn download_from_local_http_registers_model() {
        let (_dir, db, mgr) = db_and_manager("mm-download");
        let payload = {
            let mut v = GGUF_MAGIC.to_vec();
            v.extend(std::iter::repeat_n(0xABu8, 64 * 1024));
            v
        };
        let payload_len = payload.len();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = std::io::Read::read(&mut sock, &mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload_len
            );
            sock.write_all(head.as_bytes()).unwrap();
            sock.write_all(&payload).unwrap();
        });

        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress_calls = 0;
        let row = mgr
            .download(
                &db,
                &format!("http://{addr}/Qwen3-0.6B-Q4_K_M.gguf"),
                &cancel,
                &mut |_, _| progress_calls += 1,
            )
            .unwrap();

        assert_eq!(row.file_name, "Qwen3-0.6B-Q4_K_M.gguf");
        assert_eq!(row.size_bytes, payload_len as i64);
        assert_eq!(row.source, "downloaded");
        assert_eq!(row.sha256.len(), 64);
        assert!(Path::new(&row.file_path).is_file());
        assert!(progress_calls >= 1);
        assert_eq!(db.list_local_models().unwrap().len(), 1);

        // Downloading again with the same name collides.
        assert!(mgr
            .download(&db, &format!("http://{addr}/Qwen3-0.6B-Q4_K_M.gguf"), &cancel, &mut |_, _| {})
            .is_err());
    }

    #[test]
    fn download_rejects_non_gguv_payload() {
        let (_dir, db, mgr) = db_and_manager("mm-baddl");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = std::io::Read::read(&mut sock, &mut buf);
            let body = b"this is a text file not a model";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(head.as_bytes()).unwrap();
            sock.write_all(body).unwrap();
        });

        let cancel = Arc::new(AtomicBool::new(false));
        let err = mgr
            .download(&db, &format!("http://{addr}/not-a-model.gguf"), &cancel, &mut |_, _| {})
            .unwrap_err();
        assert!(err.to_string().contains("GGUF"), "{err}");
        assert!(db.list_local_models().unwrap().is_empty());
        // The partial download was cleaned up.
        assert!(!mgr.models_dir().join("not-a-model.gguf").exists());
    }

    #[test]
    fn parameter_count_formats_humanely() {
        assert_eq!(format_parameter_count(3_800_000_000), "3.8B");
        assert_eq!(format_parameter_count(630_000_000), "630M");
        assert_eq!(format_parameter_count(1234), "1234");
    }

    #[test]
    fn quantization_table_maps_common_file_types() {
        assert_eq!(quant_from_file_type(15), Some("Q4_K_M"));
        assert_eq!(quant_from_file_type(1), Some("F16"));
        assert_eq!(quant_from_file_type(999), None);
    }
}
