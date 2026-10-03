//! Bundled AI assets: first-run seeding and runtime resolution.
//!
//! The installer ships everything Ask-AI and semantic search need so the
//! product works with zero setup (offline-first, out-of-the-box AI):
//!
//! - `<resources>/models/embeddings/` — pre-fetched fastembed cache; seeded
//!   into `<data>/models/embeddings` so semantic search never needs a
//!   download and never depends on `$TMPDIR` (macOS wipes it).
//! - `<resources>/models/*.gguf` — starter model (Qwen3-0.6B Q4_K_M),
//!   imported into the library on a fresh install, activated automatically,
//!   with No-AI mode lifted for that first run only.
//! - `<resources>/llama/llama-server[.exe]` — llama.cpp CPU server, used
//!   when the user has not configured their own binary.
//!
//! Layout probes mirror `engine_runtime::find_sidecar_binary`: packaged
//! apps resolve resources directly; in dev the same assets sit under
//! `<resources>/packaging/resources/…` (the source-tree prefix).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::services::model_manager::ModelManager;

/// Resolve a bundled asset directory (`name` = "models" | "llama"):
/// packaged layout first, then the dev source-tree layout.
fn probe(resource_dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = resource_dir.join(name);
    if direct.is_dir() {
        return Some(direct);
    }
    let dev = resource_dir.join("packaging").join("resources").join(name);
    dev.is_dir().then_some(dev)
}

/// The bundled `models/` directory (embedding cache + starter GGUFs).
pub fn bundled_models_dir(resource_dir: &Path) -> Option<PathBuf> {
    probe(resource_dir, "models")
}

/// Resolve which `llama-server` to run: an explicit user path wins when it
/// exists, otherwise the bundled runtime, otherwise an actionable error.
pub fn resolve_llama_binary(preferred: &str, resource_dir: Option<&Path>) -> AppResult<PathBuf> {
    let pref = preferred.trim();
    if !pref.is_empty() && Path::new(pref).is_file() {
        return Ok(PathBuf::from(pref));
    }
    if let Some(res) = resource_dir {
        if let Some(dir) = probe(res, "llama") {
            let name = if cfg!(windows) {
                "llama-server.exe"
            } else {
                "llama-server"
            };
            let bin = dir.join(name);
            if bin.is_file() {
                log::info!(
                    target: "researchai::bundled",
                    "using bundled llama-server: {}",
                    bin.display()
                );
                return Ok(bin);
            }
        }
    }
    Err(AppError::msg(
        "llama-server was not found. Install llama.cpp and set its path in Settings → Local AI, \
         or reinstall the app to restore the bundled runtime.",
    ))
}

/// Seed bundled AI assets into the data directory on first run.
/// Never fails startup: problems are logged and retried on the next
/// launch (the marker is only written once the work has succeeded).
pub fn seed_bundled_models(resource_dir: Option<&Path>, data_dir: &Path, db: &Db) {
    let Some(res) = resource_dir else { return };
    if let Err(e) = seed_inner(res, data_dir, db) {
        log::warn!(target: "researchai::bundled", "bundled model seeding failed: {e}");
    }
}

fn seed_inner(resource_dir: &Path, data_dir: &Path, db: &Db) -> AppResult<()> {
    let Some(models_src) = probe(resource_dir, "models") else {
        return Ok(());
    };
    let models_dst = data_dir.join("models");
    let marker = models_dst.join(".bundled-seeded");
    if marker.exists() {
        return Ok(());
    }
    // Only `.gitkeep` in the bundle (a dev build without fetched assets):
    // do the work next launch instead of marking an empty probe as done.
    let emb_src = models_src.join("embeddings");
    let has_gguf = std::fs::read_dir(&models_src).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("gguf"))
        })
    });
    if !emb_src.is_dir() && !has_gguf {
        return Ok(());
    }

    // 1) Embedding cache — copy once so semantic search works offline.
    let emb_dst = models_dst.join("embeddings");
    if emb_src.is_dir() && !emb_dst.exists() {
        std::fs::create_dir_all(&models_dst)?;
        copy_dir_all(&emb_src, &emb_dst)?;
        log::info!(
            target: "researchai::bundled",
            "seeded embedding model to {}",
            emb_dst.display()
        );
    }

    // 2) Starter GGUF — only into an empty library (never over a user's
    //    choices), then auto-activate and lift No-AI mode for this fresh
    //    install. Upgrades arrive with a non-empty library and are left
    //    untouched.
    if db.list_local_models()?.is_empty() {
        let manager = ModelManager::new(data_dir);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut activated: Option<String> = None;
        if let Ok(entries) = std::fs::read_dir(&models_src) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_gguf = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("gguf"));
                if !is_gguf || !path.is_file() {
                    continue;
                }
                match manager.import_from_path(db, &path, &cancel) {
                    Ok(row) => {
                        log::info!(
                            target: "researchai::bundled",
                            "seeded bundled model {}",
                            row.file_name
                        );
                        activated.get_or_insert(row.id);
                    }
                    Err(e) => log::warn!(
                        target: "researchai::bundled",
                        "could not seed {}: {e}",
                        path.display()
                    ),
                }
            }
        }
        if let Some(id) = activated {
            let mut ai = db.get_ai_settings()?;
            if ai.active_model_id.is_none() {
                ai.active_model_id = Some(id);
                db.save_ai_settings(&ai)?;
            }
            // Out-of-the-box Ask: lift No-AI mode on this first run only.
            db.save_setting("ai_enabled", "true")?;
            log::info!(
                target: "researchai::bundled",
                "bundled model active; AI enabled out of the box"
            );
        }
    }

    std::fs::write(&marker, b"1")?;
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::{temp_dir_for, TempDir};
    use std::io::Write;

    /// Minimal GGUF: magic + filler (mirrors model_manager's fake_gguf).
    fn fake_gguf(path: &Path) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"GGUF").unwrap();
        f.write_all(&[0u8; 64]).unwrap();
    }

    fn layout(root: &Path) {
        std::fs::create_dir_all(root.join("models").join("embeddings")).unwrap();
        std::fs::write(root.join("models/embeddings/model.bin"), b"weights").unwrap();
        fake_gguf(&root.join("models/starter.gguf"));
        std::fs::write(root.join("models/notes.txt"), b"not a model").unwrap();
    }

    #[test]
    fn seed_copies_embeddings_and_activates_starter_model() {
        let dir: TempDir = temp_dir_for("bundled-fresh");
        let res = dir.path().join("res");
        let data = dir.path().join("data");
        layout(&res);
        std::fs::create_dir_all(&data).unwrap();
        let db = Db::open(&data).unwrap();

        seed_bundled_models(Some(&res), &data, &db);

        assert!(data.join("models/embeddings/model.bin").is_file());
        let models = db.list_local_models().unwrap();
        assert_eq!(models.len(), 1, "starter gguf imported exactly once");
        let ai = db.get_ai_settings().unwrap();
        assert_eq!(ai.active_model_id.as_deref(), Some(models[0].id.as_str()));
        assert!(db.load_settings().unwrap().ai_enabled, "No-AI lifted");
        assert!(data.join("models/.bundled-seeded").exists());

        // Marker makes a second start a no-op — no duplicate imports.
        seed_bundled_models(Some(&res), &data, &db);
        assert_eq!(db.list_local_models().unwrap().len(), 1);
    }

    #[test]
    fn seed_never_touches_an_existing_library() {
        let dir: TempDir = temp_dir_for("bundled-existing");
        let res = dir.path().join("res");
        let data = dir.path().join("data");
        layout(&res);
        std::fs::create_dir_all(&data).unwrap();
        let db = Db::open(&data).unwrap();

        // A user model already registered before the seed runs.
        let user_src = dir.path().join("user-model.gguf");
        fake_gguf(&user_src);
        let cancel = Arc::new(AtomicBool::new(false));
        ModelManager::new(&data)
            .import_from_path(&db, &user_src, &cancel)
            .unwrap();
        assert!(!db.load_settings().unwrap().ai_enabled);

        seed_bundled_models(Some(&res), &data, &db);

        let models = db.list_local_models().unwrap();
        assert_eq!(models.len(), 1, "bundled model not added to a live library");
        assert!(models[0].file_name.contains("user-model"));
        assert!(!db.load_settings().unwrap().ai_enabled, "settings untouched");
        // Embeddings still seeded — search benefits everyone.
        assert!(data.join("models/embeddings/model.bin").is_file());
    }

    #[test]
    fn seed_without_bundled_assets_is_a_silent_noop() {
        let dir: TempDir = temp_dir_for("bundled-none");
        let res = dir.path().join("empty");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let db = Db::open(&data).unwrap();

        seed_bundled_models(Some(&res), &data, &db);
        seed_bundled_models(None, &data, &db);

        assert!(db.list_local_models().unwrap().is_empty());
        assert!(!data.join("models").exists());
    }

    #[test]
    fn probe_finds_the_dev_source_tree_layout() {
        let dir: TempDir = temp_dir_for("bundled-devlayout");
        let res = dir.path().join("src-tauri");
        std::fs::create_dir_all(res.join("packaging/resources/models")).unwrap();

        assert_eq!(
            bundled_models_dir(&res),
            Some(res.join("packaging/resources/models"))
        );
        assert_eq!(bundled_models_dir(&dir.path().join("nowhere")), None);
    }

    #[test]
    fn resolve_prefers_user_path_then_bundled_then_errors() {
        let dir: TempDir = temp_dir_for("bundled-resolve");
        let res = dir.path().join("res");
        let llama = res.join("llama");
        std::fs::create_dir_all(&llama).unwrap();
        let bundled = llama.join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" });
        std::fs::write(&bundled, b"#!").unwrap();

        let mine = dir.path().join("my-server");
        std::fs::write(&mine, b"#!").unwrap();

        // 1) User path wins.
        assert_eq!(
            resolve_llama_binary(mine.to_str().unwrap(), Some(&res)).unwrap(),
            mine
        );
        // 2) Missing user path falls back to the bundle.
        assert_eq!(
            resolve_llama_binary("/nope/llama-server", Some(&res)).unwrap(),
            bundled
        );
        // 3) Nothing at all: actionable error.
        let err = resolve_llama_binary("", Some(&dir.path().join("bare"))).unwrap_err();
        assert!(err.to_string().contains("llama-server"));
        assert!(resolve_llama_binary("", None).is_err());
    }
}
