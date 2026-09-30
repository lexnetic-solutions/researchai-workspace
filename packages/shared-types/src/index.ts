/**
 * Shared TypeScript types for the ResearchAI IPC boundary.
 *
 * These types mirror the Rust backend contract (apps/desktop/src-tauri) and are
 * consumed by the React frontend. Every IPC payload crosses this boundary, so
 * keep this module dependency-free and serialisable.
 */

// ---------------------------------------------------------------------------
// Hardware profile (spec §2.4, §36)
// ---------------------------------------------------------------------------

export type HardwareProfile = 'light' | 'standard' | 'advanced';

export interface CpuInfo {
  readonly name: string;
  readonly cores: number;
}

export interface SystemInfo {
  readonly osName: string;
  readonly osVersion: string;
  readonly arch: string;
  readonly cpu: CpuInfo;
  /** Total physical RAM in megabytes. */
  readonly totalMemoryMb: number;
  /** Currently available physical RAM in megabytes. */
  readonly availableMemoryMb: number;
  /** Derived hardware profile used for defaults across the app. */
  readonly profile: HardwareProfile;
}

// ---------------------------------------------------------------------------
// Document ingestion status (spec §14)
// ---------------------------------------------------------------------------

export type IngestionStage =
  | 'waiting'
  | 'parsing'
  | 'ocr'
  | 'indexing'
  | 'analysing'
  | 'ready'
  | 'failed';

export interface DocumentSummary {
  readonly id: string;
  readonly projectId: string;
  readonly fileName: string;
  readonly originalPath: string;
  readonly managedPath: string | null;
  readonly documentType: string;
  readonly checksum: string;
  readonly title: string | null;
  /** Display string: "Jane Doe; John Smith" (parsed by the formatter). */
  readonly authors: string | null;
  readonly year: number | null;
  readonly doi: string | null;
  readonly journal: string | null;
  readonly volume: string | null;
  readonly issue: string | null;
  readonly pages: string | null;
  readonly publisher: string | null;
  readonly url: string | null;
  readonly refType: RefType;
  readonly indexingStatus: IngestionStage;
  readonly statusDetail: string | null;
  readonly pageCount: number | null;
  readonly language: string | null;
  readonly importedAt: string;
  readonly chunkCount: number;
}

// ---------------------------------------------------------------------------
// Citations & bibliography (Phase 5, spec §31)
// ---------------------------------------------------------------------------

export type RefType = 'article' | 'book' | 'chapter' | 'report' | 'webpage' | 'thesis';

export type CitationStyle = 'apa' | 'harvard' | 'chicago';

/** One formatted bibliography entry (deterministic, metadata-driven). */
export interface FormattedReference {
  readonly documentId: string;
  readonly style: CitationStyle;
  readonly reference: string;
  readonly inText: string;
  /** True when key fields (title/year) are missing — correct the metadata. */
  readonly incomplete: boolean;
}

/** User-correctable metadata patch; null clears a field (refType keeps). */
export interface BibliographyUpdate {
  title?: string | null;
  authors?: string | null;
  year?: number | null;
  doi?: string | null;
  journal?: string | null;
  volume?: string | null;
  issue?: string | null;
  pages?: string | null;
  publisher?: string | null;
  url?: string | null;
  refType?: RefType;
}

/** Result of an import batch (per-file errors are isolated, spec §42). */
export interface ImportSummary {
  readonly imported: number;
  readonly duplicates: number;
  readonly errors: readonly string[];
}

// ---------------------------------------------------------------------------
// Projects (spec §12)
// ---------------------------------------------------------------------------

export interface Project {
  readonly id: string;
  readonly name: string;
  readonly description: string | null;
  readonly createdAt: string;
  readonly updatedAt: string;
}

export interface ProjectStats {
  readonly projectId: string;
  readonly documentCount: number;
  readonly noteCount: number;
  readonly storageBytes: number;
}

// ---------------------------------------------------------------------------
// File storage modes (spec §13)
// ---------------------------------------------------------------------------

export type ImportMode = 'managed-copy' | 'link-original';

// ---------------------------------------------------------------------------
// Settings (spec §34: local-first, explicit user control)
// ---------------------------------------------------------------------------

export interface AppSettings {
  readonly theme: 'light' | 'dark' | 'system';
  /** Which folder holds ResearchAIData/ (managed documents, db, models). */
  readonly dataDirectory: string;
  /** Default import behaviour for new documents. */
  readonly defaultImportMode: ImportMode;
  /** OCR is enabled by default but can be disabled on low-RAM machines. */
  readonly ocrEnabled: boolean;
  /** Max concurrent parsing jobs — kept at 1 on LIGHT hardware (spec §43). */
  readonly maxConcurrentJobs: number;
  /** When true, AI-only UI features are hidden (No-AI mode, spec §35). */
  readonly aiEnabled: boolean;
}

// ---------------------------------------------------------------------------
// Diagnostics (spec §48 — first-run diagnostics)
// ---------------------------------------------------------------------------

export interface DiagnosticCheck {
  readonly id: string;
  readonly label: string;
  readonly status: 'pass' | 'warn' | 'fail';
  readonly detail: string;
}

export interface DiagnosticsReport {
  readonly system: SystemInfo;
  readonly checks: readonly DiagnosticCheck[];
  readonly dataDirectory: string;
  readonly databaseOk: boolean;
  readonly documentEngineOk: boolean;
  readonly generatedAt: string;
}

// ---------------------------------------------------------------------------
// Common result envelope for IPC calls
// ---------------------------------------------------------------------------

export interface Ok<T> {
  readonly ok: true;
  readonly data: T;
}

export interface Err {
  readonly ok: false;
  readonly error: string;
}

export type Result<T> = Ok<T> | Err;

// ---------------------------------------------------------------------------
// Retrieval (Phase 2, spec §16)
// ---------------------------------------------------------------------------

export interface SearchHit {
  readonly chunkId: string;
  readonly documentId: string;
  readonly documentName: string;
  readonly pageNumber: number | null;
  readonly sectionHeading: string | null;
  readonly text: string;
  readonly startOffset: number | null;
  readonly endOffset: number | null;
  readonly score: number;
  /** Which retrieval channels matched: "keyword" / "vector". */
  readonly matchedBy: readonly string[];
  readonly ftsRank: number | null;
  readonly vecDistance: number | null;
}

export interface RetrievalTrace {
  readonly query: string;
  readonly keywordCandidates: number;
  readonly vectorCandidates: number;
  readonly fused: number;
  readonly returned: number;
  readonly embeddingEngine: string;
  readonly embeddingCoverage: string;
  readonly warnings: readonly string[];
}

export interface SearchResponse {
  readonly hits: readonly SearchHit[];
  readonly trace: RetrievalTrace;
}

export interface RetrievalStatus {
  readonly embeddingEngine: string;
  readonly totalChunks: number;
  readonly embeddedChunks: number;
}

// ---------------------------------------------------------------------------
// Local AI — models, runtime, grounded analyses (Phase 3, spec §17)
// ---------------------------------------------------------------------------

export type ModelStatus = 'available' | 'missing' | 'downloading' | 'failed';

/** A GGUF model registered with the local model manager. */
export interface LocalModel {
  readonly id: string;
  readonly fileName: string;
  readonly filePath: string;
  readonly sizeBytes: number;
  readonly sha256: string;
  readonly parameters: string | null;
  readonly quantization: string | null;
  readonly contextTokens: number | null;
  readonly status: ModelStatus;
  readonly statusDetail: string | null;
  readonly source: 'imported' | 'downloaded';
  readonly addedAt: string;
  readonly lastUsedAt: string | null;
}

/** User-tunable AI settings (Settings → Local AI, spec §34). */
export interface AiSettings {
  readonly activeModelId: string | null;
  readonly llamaServerPath: string;
  readonly llamaServerArgs: string;
  readonly contextSize: number;
  readonly maxTokens: number;
  readonly temperature: number;
  readonly gpuLayers: number;
  readonly threads: number;
  readonly idleUnloadMinutes: number;
}

/** Discriminated runtime state from the llama-server supervisor. */
export type LoadState =
  | { readonly state: 'idle' }
  | { readonly state: 'loading' }
  | { readonly state: 'ready'; readonly port: number }
  | { readonly state: 'failed'; readonly detail: string }
  | { readonly state: 'unloaded' };

export interface RuntimeStatus {
  readonly state: LoadState;
  readonly modelFile: string | null;
  readonly pid: number | null;
  readonly idleSeconds: number | null;
}

/** Progress event emitted as `ai://model-download` during downloads. */
export interface ModelDownloadEvent {
  readonly jobId: number;
  readonly fileName: string;
  readonly downloadedBytes: number;
  readonly totalBytes: number;
  readonly state: 'running' | 'done' | 'failed' | 'cancelled';
  readonly error: string | null;
  readonly model: LocalModel | null;
}

/** One numbered evidence excerpt behind an AI answer. */
export interface AnalysisEvidence {
  readonly chunkId: string;
  readonly documentId: string;
  readonly documentName: string;
  readonly pageNumber: number | null;
  readonly sectionHeading: string | null;
  readonly text: string;
  readonly startOffset: number | null;
  readonly endOffset: number | null;
}

export type AnalysisMode =
  | 'chat'
  | 'research'
  | 'quick_read'
  | 'deep_analysis'
  | 'critical';

/** Debug trace for one analysis (mirrors the retrieval debug panel idea). */
export interface AnalysisTrace {
  readonly mode: string;
  readonly engine: string;
  readonly model: string | null;
  readonly scopeDocuments: number;
  readonly evidenceCount: number;
  readonly evidenceChars: number;
  readonly citationsUsed: readonly number[];
  readonly promptTokens: number | null;
  readonly completionTokens: number | null;
  readonly finishReason: string | null;
  readonly durationMs: number;
  readonly embeddingCoverage: string;
  readonly warnings: readonly string[];
}

export interface AnalysisResponse {
  readonly analysisId: string | null;
  readonly answer: string;
  readonly evidence: readonly AnalysisEvidence[];
  readonly trace: AnalysisTrace;
}

/** Light row for the analyses history list. */
export interface AnalysisSummary {
  readonly id: string;
  readonly analysisType: string;
  readonly question: string | null;
  readonly createdAt: string;
  readonly modelId: string | null;
}

// ---------------------------------------------------------------------------
// Evidence matrices / cross-document comparison (Phase 4, spec §18)
// ---------------------------------------------------------------------------

export interface EvidenceExcerpt {
  readonly chunkId: string;
  readonly pageNumber: number | null;
  readonly sectionHeading: string | null;
  readonly text: string;
  readonly matchedBy: readonly string[];
  readonly vecDistance: number | null;
  readonly ftsRank: number | null;
}

/** Evidence strength — describes HOW the document matched, not a truth score. */
export type EvidenceStrength = 'direct' | 'multiple' | 'indirect' | 'none';

export interface EvidenceRow {
  readonly documentId: string;
  readonly documentName: string;
  readonly strength: EvidenceStrength;
  readonly score: number | null;
  readonly excerpts: readonly EvidenceExcerpt[];
}

export interface DocFindings {
  readonly documentId: string;
  readonly documentName: string;
  readonly text: string;
  readonly citationsUsed: readonly number[];
}

export interface EvidenceTable {
  readonly question: string;
  readonly rows: readonly EvidenceRow[];
  /** Per-document AI findings; empty when no AI pass ran. */
  readonly findings: readonly DocFindings[];
  /** Cross-document AI synthesis; null when no AI pass ran. */
  readonly synthesis: string | null;
  readonly synthesisCitations: readonly number[];
}

export interface EvidenceTrace {
  readonly mode: 'deterministic' | 'ai' | string;
  readonly engine: string;
  readonly model: string | null;
  readonly scopeDocuments: number;
  readonly documentsMatched: number;
  readonly totalExcerpts: number;
  readonly strengthCounts: Readonly<Record<string, number>>;
  readonly retrievalWarnings: readonly string[];
  readonly warnings: readonly string[];
  readonly durationMs: number;
}

export interface EvidenceResponse {
  readonly tableId: string | null;
  readonly modelId: string | null;
  readonly table: EvidenceTable;
  readonly trace: EvidenceTrace;
}

export interface EvidenceSummary {
  readonly id: string;
  readonly question: string;
  readonly createdAt: string;
  readonly hasAi: boolean;
  readonly modelId: string | null;
}
