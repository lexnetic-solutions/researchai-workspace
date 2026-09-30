//! Contract-lock tests (Phase 3 hardening).
//!
//! The Rust DTOs that cross the IPC boundary must serialise with exactly the
//! keys the TypeScript contract in packages/shared-types declares
//! (camelCase). These tests lock the wire format key-for-key: if a field is
//! added or renamed here without the matching TS change (or vice versa),
//! they fail. This closes a real gap — before Phase 3 several DTOs serialised
//! snake_case while the TS side read camelCase.

#[cfg(test)]
mod tests {
    use serde::Serialize;

    /// Sorted top-level JSON keys of a serialisable value.
    fn keys<T: Serialize>(value: &T) -> Vec<String> {
        let json = serde_json::to_value(value).expect("serialise");
        let mut v: Vec<String> = json
            .as_object()
            .expect("object shape")
            .keys()
            .cloned()
            .collect();
        v.sort();
        v
    }

    fn sorted(expected: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    }

    use researchai_lib::db::{AiSettings, AnalysisRow, LocalModelRow};
    use researchai_lib::services::documents::DocumentRow;
    use researchai_lib::services::hardware::SystemInfo;
    use researchai_lib::services::llm_runtime::{LoadState, RuntimeStatus};
    use researchai_lib::services::citations::{self, CitationStyle};
    use researchai_lib::services::projects::ProjectRow;
    use researchai_lib::services::settings::{Settings, Theme};
    use researchai_lib::services::{analysis, evidence, retrieval};

    fn analysis_row() -> AnalysisRow {
        AnalysisRow {
            id: "a".into(),
            project_id: "p".into(),
            document_id: None,
            analysis_type: "chat".into(),
            model_id: None,
            prompt_version: "v1".into(),
            question: None,
            answer_text: "…".into(),
            evidence_json: "[]".into(),
            trace_json: None,
            created_at: "t".into(),
        }
    }

    #[test]
    fn project_row_keys() {
        let row = ProjectRow {
            id: "1".into(),
            name: "n".into(),
            description: None,
            created_at: "c".into(),
            updated_at: "u".into(),
        };
        assert_eq!(
            keys(&row),
            sorted(&["id", "name", "description", "createdAt", "updatedAt"])
        );
    }

    #[test]
    fn document_row_keys() {
        let row = DocumentRow {
            id: "1".into(),
            project_id: "p".into(),
            file_name: "f".into(),
            original_path: "/o".into(),
            managed_path: None,
            document_type: "txt".into(),
            checksum: "c".into(),
            title: None,
            authors: None,
            year: None,
            doi: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            publisher: None,
            url: None,
            ref_type: "article".into(),
            indexing_status: "ready".into(),
            status_detail: None,
            page_count: None,
            language: None,
            imported_at: "t".into(),
            chunk_count: 0,
        };
        assert_eq!(
            keys(&row),
            sorted(&[
                "id",
                "projectId",
                "fileName",
                "originalPath",
                "managedPath",
                "documentType",
                "checksum",
                "title",
                "authors",
                "year",
                "doi",
                "journal",
                "volume",
                "issue",
                "pages",
                "publisher",
                "url",
                "refType",
                "indexingStatus",
                "statusDetail",
                "pageCount",
                "language",
                "importedAt",
                "chunkCount",
            ])
        );
    }

    #[test]
    fn settings_keys() {
        let s = Settings::defaults("/tmp/data");
        assert_eq!(
            keys(&s),
            sorted(&[
                "theme",
                "dataDirectory",
                "defaultImportMode",
                "ocrEnabled",
                "maxConcurrentJobs",
                "aiEnabled",
            ])
        );
        assert!(matches!(s.theme, Theme::System));
    }

    #[test]
    fn search_response_keys_match_shared_types() {
        let resp = retrieval::SearchResponse {
            hits: vec![retrieval::SearchHit {
                chunk_id: "c".into(),
                document_id: "d".into(),
                document_name: "n".into(),
                page_number: Some(1),
                section_heading: None,
                text: "t".into(),
                start_offset: None,
                end_offset: None,
                score: 0.5,
                matched_by: vec!["keyword".into()],
                fts_rank: None,
                vec_distance: None,
            }],
            trace: retrieval::RetrievalTrace {
                query: "q".into(),
                keyword_candidates: 1,
                vector_candidates: 0,
                fused: 1,
                returned: 1,
                embedding_engine: "fastembed".into(),
                embedding_coverage: "0/0".into(),
                warnings: vec![],
            },
        };
        assert_eq!(
            keys(&resp.hits[0]),
            sorted(&[
                "chunkId",
                "documentId",
                "documentName",
                "pageNumber",
                "sectionHeading",
                "text",
                "startOffset",
                "endOffset",
                "score",
                "matchedBy",
                "ftsRank",
                "vecDistance",
            ])
        );
        assert_eq!(
            keys(&resp.trace),
            sorted(&[
                "query",
                "keywordCandidates",
                "vectorCandidates",
                "fused",
                "returned",
                "embeddingEngine",
                "embeddingCoverage",
                "warnings",
            ])
        );
        assert_eq!(keys(&resp), sorted(&["hits", "trace"]));
    }

    #[test]
    fn system_info_keys_match_nested_ts_contract() {
        let info = SystemInfo {
            os_name: "macOS".into(),
            os_version: "15".into(),
            arch: "aarch64".into(),
            cpu: researchai_lib::services::hardware::CpuInfo {
                name: "M-series".into(),
                cores: 8,
            },
            total_memory_mb: 16384,
            available_memory_mb: 8192,
            profile: "standard".into(),
        };
        assert_eq!(
            keys(&info),
            sorted(&[
                "osName",
                "osVersion",
                "arch",
                "cpu",
                "totalMemoryMb",
                "availableMemoryMb",
                "profile",
            ])
        );
        let cpu = serde_json::to_value(&info).unwrap()["cpu"].clone();
        let mut cpu_keys: Vec<String> = cpu.as_object().unwrap().keys().cloned().collect();
        cpu_keys.sort();
        assert_eq!(cpu_keys, vec!["cores".to_string(), "name".to_string()]);
    }

    #[test]
    fn local_model_row_keys() {
        let m = LocalModelRow {
            id: "m".into(),
            file_name: "f.gguf".into(),
            file_path: "/p".into(),
            size_bytes: 1,
            sha256: "h".into(),
            parameters: None,
            quantization: None,
            context_tokens: None,
            status: "available".into(),
            status_detail: None,
            source: "imported".into(),
            added_at: "t".into(),
            last_used_at: None,
        };
        assert_eq!(
            keys(&m),
            sorted(&[
                "id",
                "fileName",
                "filePath",
                "sizeBytes",
                "sha256",
                "parameters",
                "quantization",
                "contextTokens",
                "status",
                "statusDetail",
                "source",
                "addedAt",
                "lastUsedAt",
            ])
        );
    }

    #[test]
    fn ai_settings_roundtrips_with_camel_case_keys() {
        let s = AiSettings::default();
        assert_eq!(
            keys(&s),
            sorted(&[
                "activeModelId",
                "llamaServerPath",
                "llamaServerArgs",
                "contextSize",
                "maxTokens",
                "temperature",
                "gpuLayers",
                "threads",
                "idleUnloadMinutes",
            ])
        );
        // IPC argument direction: camelCase JSON deserialises into the struct.
        let json = serde_json::json!({
            "activeModelId": "m-1",
            "llamaServerPath": "/opt/llama-server",
            "llamaServerArgs": "",
            "contextSize": 8192,
            "maxTokens": 512,
            "temperature": 0.3,
            "gpuLayers": 12,
            "threads": 4,
            "idleUnloadMinutes": 5,
        });
        let parsed: AiSettings = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.active_model_id.as_deref(), Some("m-1"));
        assert_eq!(parsed.context_size, 8192);
        assert_eq!(parsed.gpu_layers, 12);
        assert!((parsed.temperature - 0.3).abs() < 1e-6);
    }

    #[test]
    fn analysis_dtos_match_shared_types() {
        let evidence = analysis::AnalysisEvidence {
            chunk_id: "c".into(),
            document_id: "d".into(),
            document_name: "n".into(),
            page_number: Some(2),
            section_heading: None,
            text: "t".into(),
            start_offset: None,
            end_offset: None,
        };
        assert_eq!(
            keys(&evidence),
            sorted(&[
                "chunkId",
                "documentId",
                "documentName",
                "pageNumber",
                "sectionHeading",
                "text",
                "startOffset",
                "endOffset",
            ])
        );

        let trace = analysis::AnalysisTrace {
            mode: "chat".into(),
            engine: "echo".into(),
            model: None,
            scope_documents: 0,
            evidence_count: 1,
            evidence_chars: 10,
            citations_used: vec![1],
            prompt_tokens: None,
            completion_tokens: None,
            finish_reason: None,
            duration_ms: 5,
            embedding_coverage: "0/0".into(),
            warnings: vec![],
        };
        assert_eq!(
            keys(&trace),
            sorted(&[
                "mode",
                "engine",
                "model",
                "scopeDocuments",
                "evidenceCount",
                "evidenceChars",
                "citationsUsed",
                "promptTokens",
                "completionTokens",
                "finishReason",
                "durationMs",
                "embeddingCoverage",
                "warnings",
            ])
        );

        let resp = analysis::AnalysisResponse {
            analysis_id: Some("a".into()),
            answer: "…".into(),
            evidence: vec![evidence],
            trace,
        };
        assert_eq!(
            keys(&resp),
            sorted(&["analysisId", "answer", "evidence", "trace"])
        );

        // Evidence survives the persistence round trip (deserialise side).
        let parsed: analysis::AnalysisEvidence =
            serde_json::from_str(&serde_json::to_string(&resp.evidence[0]).unwrap()).unwrap();
        assert_eq!(parsed.chunk_id, "c");
        let row = analysis_row();
        // Persistence direction: stored evidence JSON deserialises back.
        let parsed: Vec<analysis::AnalysisEvidence> =
            serde_json::from_str(&row.evidence_json).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn evidence_dtos_match_shared_types() {
        let excerpt = evidence::EvidenceExcerpt {
            chunk_id: "c".into(),
            page_number: Some(1),
            section_heading: None,
            text: "t".into(),
            matched_by: vec!["keyword".into()],
            vec_distance: None,
            fts_rank: None,
        };
        assert_eq!(
            keys(&excerpt),
            sorted(&[
                "chunkId",
                "pageNumber",
                "sectionHeading",
                "text",
                "matchedBy",
                "vecDistance",
                "ftsRank",
            ])
        );

        let row = evidence::EvidenceRow {
            document_id: "d".into(),
            document_name: "n".into(),
            strength: evidence::Strength::Direct,
            score: Some(0.5),
            excerpts: vec![excerpt],
        };
        assert_eq!(
            keys(&row),
            sorted(&["documentId", "documentName", "strength", "score", "excerpts"])
        );
        assert_eq!(
            serde_json::to_value(&row.strength).unwrap(),
            serde_json::json!("direct")
        );

        let findings = evidence::DocFindings {
            document_id: "d".into(),
            document_name: "n".into(),
            text: "s".into(),
            citations_used: vec![1],
        };
        assert_eq!(
            keys(&findings),
            sorted(&["documentId", "documentName", "text", "citationsUsed"])
        );

        let table = evidence::EvidenceTable {
            question: "q".into(),
            rows: vec![row],
            findings: vec![findings],
            synthesis: Some("s".into()),
            synthesis_citations: vec![1],
        };
        assert_eq!(
            keys(&table),
            sorted(&["question", "rows", "findings", "synthesis", "synthesisCitations"])
        );

        let trace = evidence::EvidenceTrace {
            mode: "deterministic".into(),
            engine: "deterministic".into(),
            model: None,
            scope_documents: 1,
            documents_matched: 1,
            total_excerpts: 1,
            strength_counts: [("direct".to_string(), 1usize)].into_iter().collect(),
            retrieval_warnings: vec![],
            warnings: vec![],
            duration_ms: 5,
        };
        assert_eq!(
            keys(&trace),
            sorted(&[
                "mode",
                "engine",
                "model",
                "scopeDocuments",
                "documentsMatched",
                "totalExcerpts",
                "strengthCounts",
                "retrievalWarnings",
                "warnings",
                "durationMs",
            ])
        );

        let resp = evidence::EvidenceResponse {
            table_id: None,
            model_id: None,
            table,
            trace,
        };
        assert_eq!(
            keys(&resp),
            sorted(&["tableId", "modelId", "table", "trace"])
        );

        // Persistence round trip (table_json / trace_json in migration 5).
        let json = serde_json::to_string(&resp.table).unwrap();
        let parsed: evidence::EvidenceTable = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.question, "q");
    }

    #[test]
    fn formatted_reference_keys_and_wire_shape() {
        let doc = researchai_lib::services::documents::DocumentRow {
            id: "d".into(),
            project_id: "p".into(),
            file_name: "paper.pdf".into(),
            original_path: "/p".into(),
            managed_path: None,
            document_type: "pdf".into(),
            checksum: "c".into(),
            title: Some("T".into()),
            authors: Some("A. Author".into()),
            year: Some(2024),
            doi: None,
            journal: Some("J".into()),
            volume: None,
            issue: None,
            pages: None,
            publisher: None,
            url: None,
            ref_type: "article".into(),
            indexing_status: "ready".into(),
            status_detail: None,
            page_count: None,
            language: None,
            imported_at: "t".into(),
            chunk_count: 0,
        };
        let r = citations::format_reference(&doc, CitationStyle::Apa);
        assert_eq!(
            keys(&r),
            sorted(&["documentId", "style", "reference", "inText", "incomplete"])
        );
        assert!(r.reference.contains("Author"));
        assert!(r.in_text.contains("2024"));
    }

    #[test]
    fn runtime_status_and_load_state_shapes() {
        let status = RuntimeStatus {
            state: LoadState::Ready { port: 5001 },
            model_file: Some("m.gguf".into()),
            pid: Some(42),
            idle_seconds: Some(3),
        };
        assert_eq!(
            keys(&status),
            sorted(&["state", "modelFile", "pid", "idleSeconds"])
        );
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["state"]["state"], "ready");
        assert_eq!(v["state"]["port"], 5001);

        let failed = LoadState::Failed {
            detail: "boom".into(),
        };
        let v = serde_json::to_value(&failed).unwrap();
        assert_eq!(v["state"], "failed");
        assert_eq!(v["detail"], "boom");

        assert_eq!(serde_json::to_value(LoadState::Idle).unwrap()["state"], "idle");
        assert_eq!(
            serde_json::to_value(LoadState::Unloaded).unwrap()["state"],
            "unloaded"
        );
    }
}
