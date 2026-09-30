//! SQLite persistence: schema migrations, settings store and project CRUD.
//!
//! The connection sits behind a mutex so the app state is `Sync` and can be
//! managed by Tauri (commands may run on different threads).
//!
//! Migrations are idempotent, versioned and applied at startup. Future schema
//! changes append a new `Migration` entry — never edit history (spec §47.8).

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};

use crate::error::{AppError, AppResult};
use crate::services::documents::{DocumentRow, IngestionStatus};
use crate::services::projects::ProjectRow;
use crate::services::settings::Settings;

pub struct Db {
    conn: Mutex<Connection>,
}

/// A schema version. History is append-only.
struct Migration {
    version: i64,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        // Core Phase 0 entities (spec §12).
        sql: r#"
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE settings_kv (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE INDEX idx_projects_updated ON projects(updated_at DESC);
        "#,
    },
    Migration {
        version: 2,
        // Phase 1 document model (spec §12, §14): documents with checksum
        // duplicate detection, structural sections, citation-addressable
        // chunks.
        sql: r#"
            CREATE TABLE documents (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                file_name TEXT NOT NULL,
                original_path TEXT NOT NULL,
                managed_path TEXT,
                mime_type TEXT,
                document_type TEXT NOT NULL,
                checksum TEXT NOT NULL,
                title TEXT,
                authors TEXT,
                year INTEGER,
                doi TEXT,
                imported_at TEXT NOT NULL,
                indexing_status TEXT NOT NULL DEFAULT 'waiting',
                status_detail TEXT,
                page_count INTEGER,
                language TEXT
            );

            CREATE UNIQUE INDEX idx_documents_checksum
                ON documents(project_id, checksum);

            CREATE INDEX idx_documents_project ON documents(project_id, imported_at DESC);

            CREATE TABLE document_sections (
                id TEXT PRIMARY KEY,
                document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                heading TEXT NOT NULL,
                hierarchy_level INTEGER NOT NULL,
                page_start INTEGER,
                page_end INTEGER,
                order_index INTEGER NOT NULL
            );

            CREATE INDEX idx_sections_document
                ON document_sections(document_id, order_index);

            CREATE TABLE chunks (
                id TEXT PRIMARY KEY,
                document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                section_id TEXT REFERENCES document_sections(id) ON DELETE SET NULL,
                page_number INTEGER,
                chunk_index INTEGER NOT NULL,
                text TEXT NOT NULL,
                start_offset INTEGER,
                end_offset INTEGER,
                embedding_id TEXT
            );

            CREATE INDEX idx_chunks_document
                ON chunks(document_id, chunk_index);
        "#,
    },
    Migration {
        version: 3,
        // Phase 2 retrieval (spec §16): FTS5 keyword index over chunks and
        // the vector side table. sqlite-vec virtual tables cannot be created
        // before the extension is loaded, so `chunk_embeddings` is a plain
        // table storing BLOB vectors + a shadow metadata table; KNN queries
        // brute-force in SQL via vec_distance_cosine (registered by the
        // extension). A future migration can move to vec0 virtual tables.
        sql: r#"
            CREATE VIRTUAL TABLE chunks_fts USING fts5(
                text,
                content='chunks',
                content_rowid='rowid',
                tokenize='porter unicode61'
            );

            CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, text) VALUES (new.rowid, new.text);
            END;
            CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, text)
                VALUES ('delete', old.rowid, old.text);
            END;
            CREATE TRIGGER chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, text)
                VALUES ('delete', old.rowid, old.text);
                INSERT INTO chunks_fts(rowid, text) VALUES (new.rowid, new.text);
            END;

            CREATE TABLE chunk_embeddings (
                -- NOTE: no FK to chunks(rowid) — SQLite FKs cannot target the
                -- implicit rowid. Orphan prevention relies on document_id
                -- CASCADE (documents are the deletion unit).
                chunk_rowid INTEGER PRIMARY KEY,
                document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                dimensions INTEGER NOT NULL,
                engine TEXT NOT NULL,
                vector BLOB NOT NULL
            );

            CREATE INDEX idx_chunk_embeddings_doc
                ON chunk_embeddings(document_id);

            CREATE TABLE index_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
        "#,
    },
    Migration {
        version: 4,
        // Phase 3 local AI (spec §17): registered GGUF models and the
        // persistent record of AI analyses. `local_models.file_path` is
        // UNIQUE — the same file must not be registered twice. `analyses`
        // keeps source_document_id nullable because library/project-wide
        // ask modes are not tied to one document.
        sql: r#"
            CREATE TABLE local_models (
                id TEXT PRIMARY KEY,
                file_name TEXT NOT NULL,
                file_path TEXT NOT NULL UNIQUE,
                size_bytes INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                parameters TEXT,
                quantization TEXT,
                context_tokens INTEGER,
                status TEXT NOT NULL DEFAULT 'available',
                status_detail TEXT,
                source TEXT NOT NULL DEFAULT 'imported',
                added_at TEXT NOT NULL,
                last_used_at TEXT
            );

            CREATE INDEX idx_local_models_status ON local_models(status);

            CREATE TABLE analyses (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                document_id TEXT REFERENCES documents(id) ON DELETE SET NULL,
                analysis_type TEXT NOT NULL,
                model_id TEXT,
                prompt_version TEXT NOT NULL,
                question TEXT,
                answer_text TEXT NOT NULL,
                evidence_json TEXT NOT NULL,
                trace_json TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX idx_analyses_project ON analyses(project_id, created_at DESC);
            CREATE INDEX idx_analyses_document ON analyses(document_id);
        "#,
    },
    Migration {
        version: 5,
        // Phase 4 evidence matrices (spec §18): saved cross-document
        // comparison tables. `table_json` holds the full row layout
        // (per-document excerpts + strength labels), `trace_json` the debug
        // trace. `model_id` NULL = deterministic (no AI) table.
        sql: r#"
            CREATE TABLE evidence_tables (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                question TEXT NOT NULL,
                scope_json TEXT NOT NULL,
                table_json TEXT NOT NULL,
                trace_json TEXT,
                model_id TEXT,
                prompt_version TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX idx_evidence_tables_project
                ON evidence_tables(project_id, created_at DESC);
        "#,
    },
];

impl Db {
    pub fn open(data_dir: &Path) -> AppResult<Self> {
        // Register sqlite-vec BEFORE opening the connection: auto-extensions
        // apply to connections opened after registration. Idempotent.
        crate::services::retrieval::register_vec_extension();
        let conn = Connection::open(data_dir.join("research.db"))
            .map_err(|e| AppError::msg(format!("Could not open database: {e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> AppResult<()> {
        let conn = self.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );",
        )?;
        let applied: Vec<i64> = {
            let mut stmt = conn.prepare("SELECT version FROM schema_migrations")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for m in MIGRATIONS {
            if applied.contains(&m.version) {
                continue;
            }
            conn.execute_batch(m.sql)?;
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![m.version, now_iso()],
            )?;
            log::info!(target: "researchai::db", "applied migration {}", m.version);
        }
        Ok(())
    }

    // -- settings -----------------------------------------------------------

    pub fn load_settings(&self) -> AppResult<Settings> {
        let conn = self.lock();
        let map: std::collections::HashMap<String, String> = {
            let mut stmt = conn.prepare("SELECT key, value FROM settings_kv")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter().collect()
        };
        Ok(Settings::from_map(&map))
    }

    pub fn save_setting(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO settings_kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    // -- projects -----------------------------------------------------------

    pub fn list_projects(&self) -> AppResult<Vec<ProjectRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, created_at, updated_at
             FROM projects ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ProjectRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn create_project(&self, name: &str, description: Option<&str>) -> AppResult<ProjectRow> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::msg("Project name must not be empty."));
        }
        let description = description.map(str::trim).filter(|s| !s.is_empty());

        let id = uuid::Uuid::new_v4().to_string();
        let now = now_iso();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO projects (id, name, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, name, description, now],
        )?;
        Ok(ProjectRow {
            id,
            name: name.to_string(),
            description: description.map(str::to_string),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn delete_project(&self, id: &str) -> AppResult<()> {
        let conn = self.lock();
        let n = conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(AppError::msg("Project not found."));
        }
        Ok(())
    }

    // -- documents (Phase 1) -------------------------------------------------

    pub fn find_document_by_checksum(
        &self,
        project_id: &str,
        checksum: &str,
    ) -> AppResult<Option<DocumentRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.project_id, d.file_name, d.original_path, d.managed_path,
                    d.document_type, d.checksum, d.title, d.indexing_status, d.status_detail,
                    d.page_count, d.language, d.imported_at,
                    (SELECT COUNT(*) FROM chunks c WHERE c.document_id = d.id) AS chunk_count
             FROM documents d
             WHERE d.project_id = ?1 AND d.checksum = ?2",
        )?;
        let row = stmt
            .query_row(rusqlite::params![project_id, checksum], row_to_document)
            .optional()
            .map_err(AppError::from)?;
        Ok(row)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_document(&self, doc: DocumentRow) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO documents (id, project_id, file_name, original_path, managed_path,
                                    mime_type, document_type, checksum, title, authors, year,
                                    doi, imported_at, indexing_status, status_detail,
                                    page_count, language)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, NULL, NULL, NULL, ?9, ?10, ?11,
                     ?12, ?13)
             ON CONFLICT(project_id, checksum) DO NOTHING",
            rusqlite::params![
                doc.id,
                doc.project_id,
                doc.file_name,
                doc.original_path,
                doc.managed_path,
                doc.document_type,
                doc.checksum,
                doc.title,
                doc.imported_at,
                doc.indexing_status,
                doc.status_detail,
                doc.page_count,
                doc.language,
            ],
        )?;
        Ok(())
    }

    pub fn list_documents(&self, project_id: &str) -> AppResult<Vec<DocumentRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.project_id, d.file_name, d.original_path, d.managed_path,
                    d.document_type, d.checksum, d.title, d.indexing_status, d.status_detail,
                    d.page_count, d.language, d.imported_at,
                    (SELECT COUNT(*) FROM chunks c WHERE c.document_id = d.id) AS chunk_count
             FROM documents d
             WHERE d.project_id = ?1
             ORDER BY d.imported_at DESC",
        )?;
        let rows = stmt
            .query_map([project_id], row_to_document)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_document(&self, document_id: &str) -> AppResult<DocumentRow> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.project_id, d.file_name, d.original_path, d.managed_path,
                    d.document_type, d.checksum, d.title, d.indexing_status, d.status_detail,
                    d.page_count, d.language, d.imported_at,
                    (SELECT COUNT(*) FROM chunks c WHERE c.document_id = d.id) AS chunk_count
             FROM documents d WHERE d.id = ?1",
        )?;
        stmt.query_row([document_id], row_to_document)
            .map_err(AppError::from)
    }

    pub fn set_document_status(
        &self,
        document_id: &str,
        status: IngestionStatus,
        detail: Option<&str>,
    ) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE documents SET indexing_status = ?2, status_detail = ?3 WHERE id = ?1",
            rusqlite::params![document_id, status.as_str(), detail],
        )?;
        Ok(())
    }

    pub fn update_document_metadata(
        &self,
        document_id: &str,
        title: Option<&str>,
        page_count: Option<i64>,
        language: Option<&str>,
        status: IngestionStatus,
    ) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE documents SET title = COALESCE(?2, title), page_count = ?3,
                    language = ?4, indexing_status = ?5
             WHERE id = ?1",
            rusqlite::params![document_id, title, page_count, language, status.as_str()],
        )?;
        Ok(())
    }

    /// Replace all sections and chunks for a document (transactional).
    pub fn replace_sections_and_chunks(
        &self,
        document_id: &str,
        parsed: &crate::services::documents::EngineParseResponse,
    ) -> AppResult<()> {
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM chunks WHERE document_id = ?1", [document_id])?;
        tx.execute("DELETE FROM document_sections WHERE document_id = ?1", [document_id])?;

        let mut section_ids: Vec<String> = Vec::with_capacity(parsed.sections.len());
        for s in &parsed.sections {
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO document_sections (id, document_id, heading, hierarchy_level,
                                                page_start, page_end, order_index)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![id, document_id, s.heading, s.level, s.page_start, s.page_end, s.order_index],
            )?;
            section_ids.push(id);
        }

        for (index, b) in parsed.blocks.iter().enumerate() {
            let section_id = parsed
                .sections
                .get(b.section_index)
                .and_then(|_| section_ids.get(b.section_index))
                .cloned();
            tx.execute(
                "INSERT INTO chunks (id, document_id, section_id, page_number, chunk_index,
                                     text, start_offset, end_offset)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    document_id,
                    section_id,
                    b.page,
                    index as i64,
                    b.text,
                    b.start_offset,
                    b.end_offset,
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Oldest `waiting` document, or None when the queue is empty.
    pub fn next_waiting_document(&self) -> AppResult<Option<DocumentRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.project_id, d.file_name, d.original_path, d.managed_path,
                    d.document_type, d.checksum, d.title, d.indexing_status, d.status_detail,
                    d.page_count, d.language, d.imported_at,
                    (SELECT COUNT(*) FROM chunks c WHERE c.document_id = d.id) AS chunk_count
             FROM documents d
             WHERE d.indexing_status = 'waiting'
             ORDER BY d.imported_at ASC
             LIMIT 1",
        )?;
        stmt.query_row([], row_to_document)
            .optional()
            .map_err(AppError::from)
    }

    pub fn delete_document(&self, document_id: &str) -> AppResult<()> {
        let conn = self.lock();
        // Fetch managed path first so the caller can clean the file.
        let managed: Option<String> = conn
            .query_row(
                "SELECT managed_path FROM documents WHERE id = ?1",
                [document_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::from)?
            .flatten();
        if let Some(path) = managed {
            let _ = std::fs::remove_file(path); // missing file is fine
        }
        let n = conn.execute("DELETE FROM documents WHERE id = ?1", [document_id])?;
        if n == 0 {
            return Err(AppError::msg("Document not found."));
        }
        Ok(())
    }

    /// Assembled readable text for the viewer: chunks joined with page
    /// markers so citations can reference `page` precisely.
    pub fn get_document_text(&self, document_id: &str) -> AppResult<String> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT page_number, text FROM chunks
             WHERE document_id = ?1 ORDER BY chunk_index",
        )?;
        let rows = stmt
            .query_map([document_id], |row| {
                Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut out = String::new();
        let mut last_page: Option<i64> = None;
        for (page, text) in rows {
            if page != last_page {
                if let Some(p) = page {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&format!("[Page {p}]\n"));
                }
                last_page = page;
            }
            out.push_str(&text);
            out.push_str("\n\n");
        }
        Ok(out.trim().to_string())
    }

    // -- local AI models (Phase 3, spec §17) ---------------------------------

    pub fn list_local_models(&self) -> AppResult<Vec<LocalModelRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, file_name, file_path, size_bytes, sha256, parameters, quantization,
                    context_tokens, status, status_detail, source, added_at, last_used_at
             FROM local_models ORDER BY added_at DESC",
        )?;
        let rows = stmt
            .query_map([], row_to_local_model)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_local_model(&self, model_id: &str) -> AppResult<LocalModelRow> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, file_name, file_path, size_bytes, sha256, parameters, quantization,
                    context_tokens, status, status_detail, source, added_at, last_used_at
             FROM local_models WHERE id = ?1",
        )?;
        stmt.query_row([model_id], row_to_local_model)
            .map_err(AppError::from)
    }

    pub fn insert_local_model(&self, m: &LocalModelRow) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO local_models (id, file_name, file_path, size_bytes, sha256, parameters,
                                       quantization, context_tokens, status, status_detail,
                                       source, added_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(file_path) DO NOTHING",
            rusqlite::params![
                m.id, m.file_name, m.file_path, m.size_bytes, m.sha256, m.parameters,
                m.quantization, m.context_tokens, m.status, m.status_detail, m.source,
                m.added_at, m.last_used_at,
            ],
        )?;
        Ok(())
    }

    pub fn update_local_model_status(
        &self,
        model_id: &str,
        status: &str,
        detail: Option<&str>,
    ) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE local_models SET status = ?2, status_detail = ?3 WHERE id = ?1",
            rusqlite::params![model_id, status, detail],
        )?;
        Ok(())
    }

    pub fn touch_local_model(&self, model_id: &str) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE local_models SET last_used_at = ?2 WHERE id = ?1",
            rusqlite::params![model_id, now_iso()],
        )?;
        Ok(())
    }

    /// Remove the registry row (never the file on disk — callers decide).
    pub fn delete_local_model(&self, model_id: &str) -> AppResult<()> {
        let conn = self.lock();
        let n = conn.execute("DELETE FROM local_models WHERE id = ?1", [model_id])?;
        if n == 0 {
            return Err(AppError::msg("Model not found."));
        }
        Ok(())
    }

    // -- AI analyses (Phase 3) ------------------------------------------------

    pub fn insert_analysis(&self, a: &AnalysisRow) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO analyses (id, project_id, document_id, analysis_type, model_id,
                                   prompt_version, question, answer_text, evidence_json,
                                   trace_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                answer_text = excluded.answer_text,
                evidence_json = excluded.evidence_json,
                trace_json = excluded.trace_json,
                created_at = excluded.created_at,
                model_id = excluded.model_id,
                prompt_version = excluded.prompt_version",
            rusqlite::params![
                a.id, a.project_id, a.document_id, a.analysis_type, a.model_id,
                a.prompt_version, a.question, a.answer_text, a.evidence_json,
                a.trace_json, a.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_analysis(&self, analysis_id: &str) -> AppResult<AnalysisRow> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, document_id, analysis_type, model_id, prompt_version,
                    question, answer_text, evidence_json, trace_json, created_at
             FROM analyses WHERE id = ?1",
        )?;
        stmt
            .query_row([analysis_id], row_to_analysis)
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("Analysis not found."))
    }

    pub fn list_analyses(&self, project_id: &str, limit: usize) -> AppResult<Vec<AnalysisRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, document_id, analysis_type, model_id, prompt_version,
                    question, answer_text, evidence_json, trace_json, created_at
             FROM analyses WHERE project_id = ?1
             ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![project_id, limit as i64],
                row_to_analysis,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// AI-specific settings (model selection, llama-server binary, context
    /// sizing) — separate keys so the core Settings struct stays small.
    pub fn get_ai_settings(&self) -> AppResult<AiSettings> {
        let conn = self.lock();
        let map: std::collections::HashMap<String, String> = {
            let mut stmt = conn
                .prepare("SELECT key, value FROM settings_kv WHERE key LIKE 'ai.%'")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter().collect()
        };
        Ok(AiSettings::from_map(&map))
    }

    pub fn save_ai_settings(&self, s: &AiSettings) -> AppResult<()> {
        for (k, v) in s.to_map() {
            self.save_setting(k, &v)?;
        }
        Ok(())
    }

    // -- evidence tables (Phase 4, spec §18) ----------------------------------

    pub fn insert_evidence_table(&self, t: &EvidenceTableRow) -> AppResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO evidence_tables (id, project_id, question, scope_json,
                                          table_json, trace_json, model_id,
                                          prompt_version, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                question = excluded.question,
                scope_json = excluded.scope_json,
                table_json = excluded.table_json,
                trace_json = excluded.trace_json,
                model_id = excluded.model_id,
                prompt_version = excluded.prompt_version,
                created_at = excluded.created_at",
            rusqlite::params![
                t.id, t.project_id, t.question, t.scope_json, t.table_json,
                t.trace_json, t.model_id, t.prompt_version, t.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_evidence_table(&self, table_id: &str) -> AppResult<EvidenceTableRow> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, question, scope_json, table_json, trace_json,
                    model_id, prompt_version, created_at
             FROM evidence_tables WHERE id = ?1",
        )?;
        stmt.query_row([table_id], row_to_evidence_table)
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("Evidence table not found."))
    }

    pub fn list_evidence_tables(
        &self,
        project_id: &str,
        limit: usize,
    ) -> AppResult<Vec<EvidenceTableRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, question, scope_json, table_json, trace_json,
                    model_id, prompt_version, created_at
             FROM evidence_tables WHERE project_id = ?1
             ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![project_id, limit as i64],
                row_to_evidence_table,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_evidence_table(&self, table_id: &str) -> AppResult<()> {
        let conn = self.lock();
        let n = conn.execute("DELETE FROM evidence_tables WHERE id = ?1", [table_id])?;
        if n == 0 {
            return Err(AppError::msg("Evidence table not found."));
        }
        Ok(())
    }

    /// Connection access for services (retrieval) in the same crate.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("database lock poisoned")
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Public timestamp helper for services that insert rows.
pub fn now_iso_pub() -> String {
    now_iso()
}

fn row_to_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentRow> {
    Ok(DocumentRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        file_name: row.get(2)?,
        original_path: row.get(3)?,
        managed_path: row.get(4)?,
        document_type: row.get(5)?,
        checksum: row.get(6)?,
        title: row.get(7)?,
        indexing_status: row.get(8)?,
        status_detail: row.get(9)?,
        page_count: row.get(10)?,
        language: row.get(11)?,
        imported_at: row.get(12)?,
        chunk_count: row.get(13)?,
    })
}

fn row_to_local_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalModelRow> {
    Ok(LocalModelRow {
        id: row.get(0)?,
        file_name: row.get(1)?,
        file_path: row.get(2)?,
        size_bytes: row.get(3)?,
        sha256: row.get(4)?,
        parameters: row.get(5)?,
        quantization: row.get(6)?,
        context_tokens: row.get(7)?,
        status: row.get(8)?,
        status_detail: row.get(9)?,
        source: row.get(10)?,
        added_at: row.get(11)?,
        last_used_at: row.get(12)?,
    })
}

fn row_to_evidence_table(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceTableRow> {
    Ok(EvidenceTableRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        question: row.get(2)?,
        scope_json: row.get(3)?,
        table_json: row.get(4)?,
        trace_json: row.get(5)?,
        model_id: row.get(6)?,
        prompt_version: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn row_to_analysis(row: &rusqlite::Row<'_>) -> rusqlite::Result<AnalysisRow> {
    Ok(AnalysisRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        document_id: row.get(2)?,
        analysis_type: row.get(3)?,
        model_id: row.get(4)?,
        prompt_version: row.get(5)?,
        question: row.get(6)?,
        answer_text: row.get(7)?,
        evidence_json: row.get(8)?,
        trace_json: row.get(9)?,
        created_at: row.get(10)?,
    })
}

/// A GGUF model registered with the local model manager (spec §17).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelRow {
    pub id: String,
    pub file_name: String,
    /// Absolute path of the GGUF file inside the managed models/ directory.
    pub file_path: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub parameters: Option<String>,
    pub quantization: Option<String>,
    pub context_tokens: Option<i64>,
    /// "available" | "missing" | "downloading" | "failed"
    pub status: String,
    pub status_detail: Option<String>,
    /// "imported" | "downloaded"
    pub source: String,
    pub added_at: String,
    pub last_used_at: Option<String>,
}

/// A saved evidence matrix (Phase 4, spec §18).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceTableRow {
    pub id: String,
    pub project_id: String,
    pub question: String,
    /// JSON array of scoped document ids ([] = whole project).
    pub scope_json: String,
    /// JSON-serialized `EvidenceTable` (evidence.rs).
    pub table_json: String,
    /// JSON-serialized `EvidenceTrace` (evidence.rs); optional.
    pub trace_json: Option<String>,
    /// NULL = deterministic table (no AI pass).
    pub model_id: Option<String>,
    pub prompt_version: String,
    pub created_at: String,
}

/// A persisted AI analysis (spec §17) with its evidence list and trace.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnalysisRow {
    pub id: String,
    pub project_id: String,
    pub document_id: Option<String>,
    pub analysis_type: String,
    pub model_id: Option<String>,
    pub prompt_version: String,
    pub question: Option<String>,
    pub answer_text: String,
    /// JSON-serialized `Vec<AnalysisEvidence>` (analysis.rs).
    pub evidence_json: String,
    /// JSON-serialized `AnalysisTrace` (analysis.rs); optional.
    pub trace_json: Option<String>,
    pub created_at: String,
}

/// User-tunable AI settings beyond the core Settings struct (spec §17/§34).
/// Crosses the IPC boundary in both directions (ai_save_settings argument).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    /// Which registered model loads when the AI engine starts.
    pub active_model_id: Option<String>,
    /// Path to the llama.cpp `llama-server` binary.
    pub llama_server_path: String,
    /// Extra CLI args passed through to llama-server (power users).
    pub llama_server_args: String,
    pub context_size: u32,
    pub max_tokens: u32,
    pub temperature: f32,
    pub gpu_layers: u32,
    pub threads: u32,
    /// Auto-unload the model after this many idle minutes (0 = never).
    pub idle_unload_minutes: u32,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            active_model_id: None,
            llama_server_path: String::new(),
            llama_server_args: String::new(),
            // 4096 fits the LIGHT profile budget alongside the OS + app.
            context_size: 4096,
            max_tokens: 1024,
            temperature: 0.2,
            gpu_layers: 0,
            threads: 0, // 0 = let llama.cpp use its default
            idle_unload_minutes: 10,
        }
    }
}

impl AiSettings {
    fn from_map(map: &std::collections::HashMap<String, String>) -> Self {
        let get = |k: &str| map.get(&format!("ai.{k}")).cloned();
        let get_u32 = |k: &str| get(k).and_then(|v| v.parse().ok());
        let get_f32 = |k: &str| get(k).and_then(|v| v.parse().ok());
        let d = Self::default();
        Self {
            active_model_id: get("active_model_id").filter(|s| !s.is_empty()),
            llama_server_path: get("llama_server_path").unwrap_or_default(),
            llama_server_args: get("llama_server_args").unwrap_or_default(),
            context_size: get_u32("context_size").unwrap_or(d.context_size),
            max_tokens: get_u32("max_tokens").unwrap_or(d.max_tokens),
            temperature: get_f32("temperature").unwrap_or(d.temperature),
            gpu_layers: get_u32("gpu_layers").unwrap_or(d.gpu_layers),
            threads: get_u32("threads").unwrap_or(d.threads),
            idle_unload_minutes: get_u32("idle_unload_minutes").unwrap_or(d.idle_unload_minutes),
        }
    }

    fn to_map(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "ai.active_model_id",
                self.active_model_id.clone().unwrap_or_default(),
            ),
            ("ai.llama_server_path", self.llama_server_path.clone()),
            ("ai.llama_server_args", self.llama_server_args.clone()),
            ("ai.context_size", self.context_size.to_string()),
            ("ai.max_tokens", self.max_tokens.to_string()),
            ("ai.temperature", self.temperature.to_string()),
            ("ai.gpu_layers", self.gpu_layers.to_string()),
            ("ai.threads", self.threads.to_string()),
            (
                "ai.idle_unload_minutes",
                self.idle_unload_minutes.to_string(),
            ),
        ]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Shared temp-dir helper for tests in other modules.
    pub(crate) fn temp_dir_for(label: &str) -> TempDir {
        TempDir::new_with_label(label)
    }

    fn temp_db() -> (TempDir, Db) {
        let dir = TempDir::new();
        let db = Db::open(dir.path()).expect("open db");
        (dir, db)
    }

    #[test]
    fn migrations_are_idempotent() {
        let (_dir, db) = temp_db();
        db.migrate().expect("first migrate");
        db.migrate().expect("second migrate");
    }

    #[test]
    fn project_crud_roundtrip() {
        let (_dir, db) = temp_db();
        let p = db.create_project("Thesis", Some("Coastal adaptation")).unwrap();
        assert_eq!(db.list_projects().unwrap().len(), 1);
        db.delete_project(&p.id).unwrap();
        assert!(db.list_projects().unwrap().is_empty());
        assert!(db.delete_project("missing").is_err());
        assert!(db.create_project("   ", None).is_err());
    }

    #[test]
    fn settings_roundtrip() {
        let (_dir, db) = temp_db();
        db.save_setting("theme", "dark").unwrap();
        db.save_setting("theme", "light").unwrap();
        let s = db.load_settings().unwrap();
        assert_eq!(s.theme, crate::services::settings::Theme::Light);
    }

    #[test]
    fn local_model_crud_roundtrip() {
        let (_dir, db) = temp_db();
        let m = crate::db::LocalModelRow {
            id: "m-1".into(),
            file_name: "model.gguf".into(),
            file_path: "/models/model.gguf".into(),
            size_bytes: 1234,
            sha256: "deadbeef".into(),
            parameters: Some("3.8B".into()),
            quantization: Some("Q4_K_M".into()),
            context_tokens: Some(4096),
            status: "available".into(),
            status_detail: None,
            source: "imported".into(),
            added_at: crate::db::now_iso_pub(),
            last_used_at: None,
        };
        db.insert_local_model(&m).unwrap();
        let listed = db.list_local_models().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file_name, "model.gguf");

        db.update_local_model_status("m-1", "failed", Some("boom")).unwrap();
        assert_eq!(db.get_local_model("m-1").unwrap().status, "failed");

        db.touch_local_model("m-1").unwrap();
        assert!(db.get_local_model("m-1").unwrap().last_used_at.is_some());

        // Duplicate file_path resolves via ON CONFLICT DO NOTHING.
        let mut dup = m.clone();
        dup.id = "m-2".into();
        db.insert_local_model(&dup).unwrap();
        assert_eq!(db.list_local_models().unwrap().len(), 1);

        db.delete_local_model("m-1").unwrap();
        assert!(db.get_local_model("m-1").is_err());
    }

    #[test]
    fn analysis_persist_and_list() {
        let (_dir, db) = temp_db();
        let project = db.create_project("P", None).unwrap();
        let a = crate::db::AnalysisRow {
            id: "a-1".into(),
            project_id: project.id.clone(),
            document_id: None,
            analysis_type: "chat".into(),
            model_id: Some("m-1".into()),
            prompt_version: "v1".into(),
            question: Some("Q?".into()),
            answer_text: "A.".into(),
            evidence_json: "[]".into(),
            trace_json: None,
            created_at: crate::db::now_iso_pub(),
        };
        db.insert_analysis(&a).unwrap();
        let listed = db.list_analyses(&project.id, 10).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].answer_text, "A.");
        db.get_analysis("a-1").unwrap();
        assert!(db.get_analysis("missing").is_err());
    }

    #[test]
    fn evidence_table_persist_and_list() {
        let (_dir, db) = temp_db();
        let project = db.create_project("P", None).unwrap();
        let row = crate::db::EvidenceTableRow {
            id: "et-1".into(),
            project_id: project.id.clone(),
            question: "Which deltas retreat fastest?".into(),
            scope_json: "[]".into(),
            table_json: "{\"rows\":[]}".into(),
            trace_json: None,
            model_id: None,
            prompt_version: "v1".into(),
            created_at: crate::db::now_iso_pub(),
        };
        db.insert_evidence_table(&row).unwrap();
        let listed = db.list_evidence_tables(&project.id, 10).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].question, "Which deltas retreat fastest?");
        db.get_evidence_table("et-1").unwrap();
        assert!(db.get_evidence_table("missing").is_err());
        db.delete_evidence_table("et-1").unwrap();
        assert!(db.get_evidence_table("et-1").is_err());
        assert!(db.delete_evidence_table("et-1").is_err());
    }

    #[test]
    fn ai_settings_roundtrip() {
        let (_dir, db) = temp_db();
        let defaults = db.get_ai_settings().unwrap();
        assert_eq!(defaults.context_size, 4096);
        assert!(defaults.active_model_id.is_none());

        let mut s = defaults;
        s.active_model_id = Some("m-1".into());
        s.llama_server_path = "/opt/llama-server".into();
        s.temperature = 0.5;
        db.save_ai_settings(&s).unwrap();
        let loaded = db.get_ai_settings().unwrap();
        assert_eq!(loaded.active_model_id.as_deref(), Some("m-1"));
        assert_eq!(loaded.llama_server_path, "/opt/llama-server");
        assert!((loaded.temperature - 0.5).abs() < 1e-6);
    }

    #[test]
    fn concurrent_access_is_safe() {
        let db = std::sync::Arc::new(temp_db());
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let db = std::sync::Arc::clone(&db);
                std::thread::spawn(move || {
                    db.1.create_project(&format!("P{i}"), None).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(db.1.list_projects().unwrap().len(), 4);
    }

    /// Minimal scoped temp-dir helper so the crate has no dev-dep on tempfile.
    pub(crate) struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub(crate) fn new() -> Self {
            Self::new_with_label("db")
        }

        pub(crate) fn new_with_label(label: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "researchai-test-{label}-{}-{}",
                std::process::id(),
                n
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
