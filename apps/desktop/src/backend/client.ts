import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  AiSettings,
  AnalysisEvidence,
  AnalysisMode,
  AnalysisResponse,
  AnalysisSummary,
  AppSettings,
  BibliographyUpdate,
  CitationStyle,
  DiagnosticsReport,
  DocumentSummary,
  EvidenceResponse,
  EvidenceSummary,
  EvidenceTable,
  EvidenceTrace,
  ExportFile,
  ExportFormat,
  ExportKind,
  ExportKindCapability,
  ExportProgressEvent,
  ExportResult,
  ExportStats,
  FormattedReference,
  ImportMode,
  ImportSummary,
  LocalModel,
  ModelDownloadEvent,
  NarrationKind,
  NarrationResult,
  Project,
  Result,
  RetrievalStatus,
  RuntimeStatus,
  SearchResponse,
  SttSettings,
  SttStatus,
  SystemInfo,
  TranscriptionJobResult,
  TtsSettings,
  TtsStatus,
} from '@researchai/shared-types';

/**
 * Backend access layer.
 *
 * In the packaged desktop app all calls cross the Tauri IPC boundary into the
 * Rust core. When the frontend is served in a plain browser (e.g. the
 * Freebuff preview, or `pnpm dev` for UI work), we fall back to an in-memory
 * mock so the full UI can be exercised without the native shell.
 */

/** True when running inside the Tauri shell. */
export const isNative = '__TAURI_INTERNALS__' in window;

export async function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (isNative) {
    return tauriInvoke<T>(cmd, args);
  }
  return mockInvoke<T>(cmd, args);
}

// ---------------------------------------------------------------------------
// Domain API surface
// ---------------------------------------------------------------------------

export const backend = {
  listProjects: () => invoke<Project[]>('list_projects'),
  createProject: (name: string, description?: string) =>
    invoke<Project>('create_project', { name, description: description ?? null }),
  deleteProject: (id: string) => invoke<void>('delete_project', { id }),

  getSettings: () => invoke<AppSettings>('get_settings'),
  setTheme: (theme: 'light' | 'dark' | 'system') =>
    invoke<AppSettings>('set_theme', { theme }),
  setAiEnabled: (enabled: boolean) => invoke<AppSettings>('set_ai_enabled', { enabled }),
  aiPickModelFile: () => invoke<string | null>('ai_pick_model_file'),

  getSystemInfo: () => invoke<SystemInfo>('get_system_info'),
  runDiagnostics: () => invoke<DiagnosticsReport>('run_diagnostics'),

  pickFolder: () => invoke<string | null>('pick_folder'),
  pickDocuments: () => invoke<string[] | null>('pick_documents'),

  listDocuments: (projectId: string) =>
    invoke<DocumentSummary[]>('list_documents', { projectId }),
  importDocuments: (projectId: string, paths: string[], mode: ImportMode) =>
    invoke<ImportSummary>('import_documents', { projectId, paths, mode }),
  retryDocument: (documentId: string) =>
    invoke<void>('retry_document', { documentId }),
  deleteDocument: (documentId: string) =>
    invoke<void>('delete_document', { documentId }),
  getDocumentText: (documentId: string) =>
    invoke<string>('get_document_text', { documentId }),

  // -- Citations & bibliography (Phase 5, spec §31) -------------------------
  updateBibliography: (documentId: string, update: BibliographyUpdate) =>
    invoke<void>('update_document_bibliography', { documentId, update }),
  bibliographyList: (projectId: string, style: CitationStyle) =>
    invoke<FormattedReference[]>('bibliography_list', { projectId, style }),

  // -- Academic exports (Phase 6, spec §32) ---------------------------------
  exportCapabilities: () =>
    invoke<{ formats: ExportFormat[]; kinds: ExportKindCapability[] }>(
      'export_capabilities',
    ),
  exportDocument: (
    projectId: string,
    kind: ExportKind,
    sourceId: string | null,
    format: ExportFormat,
    style: CitationStyle = 'apa',
  ) =>
    invoke<ExportResult>('export_document', {
      projectId,
      kind,
      sourceId,
      format,
      style,
    }),
  exportBibliography: (projectId: string, format: 'bibtex' | 'ris') =>
    invoke<ExportResult>('export_bibliography', { projectId, format }),
  listExports: () => invoke<ExportFile[]>('list_exports'),
  exportsStats: () => invoke<ExportStats>('exports_stats'),
  deleteExport: (path: string) => invoke<ExportStats>('delete_export', { path }),
  revealPath: (path: string) => invoke<string>('reveal_path', { path }),

  searchLibrary: (query: string, documentIds: string[] = [], limit = 12) =>
    invoke<SearchResponse>('search_library', { query, documentIds, limit }),
  retrievalStatus: () => invoke<RetrievalStatus>('retrieval_status'),

  // -- Local AI (Phase 3, spec §17) ----------------------------------------
  aiListModels: () => invoke<LocalModel[]>('ai_list_models'),
  aiAddModel: (path: string) => invoke<LocalModel>('ai_add_model', { path }),
  aiDownloadModel: (url: string) => invoke<number>('ai_download_model', { url }),
  aiCancelDownload: (jobId: number) => invoke<boolean>('ai_cancel_download', { jobId }),
  aiDeleteModel: (modelId: string, removeFile = false) =>
    invoke<void>('ai_delete_model', { modelId, removeFile }),
  aiGetSettings: () => invoke<AiSettings>('ai_get_settings'),
  aiSaveSettings: (settings: AiSettings) =>
    invoke<AiSettings>('ai_save_settings', { settings }),
  aiRuntimeStatus: () => invoke<RuntimeStatus>('ai_runtime_status'),
  aiLoadModel: () => invoke<RuntimeStatus>('ai_load_model'),
  aiUnloadModel: () => invoke<RuntimeStatus>('ai_unload_model'),
  aiAsk: (projectId: string, documentIds: string[], mode: AnalysisMode, question: string) =>
    invoke<AnalysisResponse>('ai_ask', { projectId, documentIds, mode, question }),
  aiAskStream: (projectId: string, documentIds: string[], mode: AnalysisMode, question: string) =>
    invoke<AnalysisResponse>('ai_ask_stream', { projectId, documentIds, mode, question }),
  aiListAnalyses: (projectId: string, limit = 50) =>
    invoke<AnalysisSummary[]>('ai_list_analyses', { projectId, limit }),
  aiGetAnalysis: (analysisId: string) =>
    invoke<AnalysisResponse>('ai_get_analysis', { analysisId }),

  // -- Evidence matrices (Phase 4, spec §18) -------------------------------
  evidenceBuild: (
    projectId: string,
    documentIds: string[],
    question: string,
    withAi: boolean,
  ) =>
    invoke<EvidenceResponse>('evidence_build', {
      projectId,
      documentIds,
      question,
      withAi,
    }),
  evidenceSave: (
    projectId: string,
    question: string,
    scopeDocumentIds: string[],
    table: EvidenceTable,
    trace: EvidenceTrace,
    modelId: string | null,
  ) =>
    invoke<string>('evidence_save', {
      projectId,
      question,
      scopeDocumentIds,
      table,
      trace,
      modelId,
    }),
  evidenceList: (projectId: string, limit = 50) =>
    invoke<EvidenceSummary[]>('evidence_list', { projectId, limit }),
  evidenceGet: (tableId: string) =>
    invoke<EvidenceResponse>('evidence_get', { tableId }),  evidenceDelete: (tableId: string) => invoke<void>('evidence_delete', { tableId }),

  // Speech-to-text (Phase 7) ----------------------------------------------
  sttCheck: () => invoke<SttStatus>('stt_check'),
  sttGetSettings: () => invoke<SttSettings>('stt_get_settings'),
  sttSaveSettings: (settings: SttSettings) =>
    invoke<SttSettings>('stt_save_settings', { settings }),
  sttTranscribe: (projectId: string, audioPath: string) =>
    invoke<TranscriptionJobResult>('stt_transcribe', { projectId, audioPath }),

  // Text-to-speech (Phase 8) -----------------------------------------------
  ttsCheck: () => invoke<TtsStatus>('tts_check'),
  ttsGetSettings: () => invoke<TtsSettings>('tts_get_settings'),
  ttsSaveSettings: (settings: TtsSettings) =>
    invoke<TtsSettings>('tts_save_settings', { settings }),
  ttsSpeakDocument: (documentId: string) =>
    invoke<NarrationResult>('tts_speak_document', { documentId }),
  ttsNarrate: (documentId: string, kind: NarrationKind) =>
    invoke<NarrationResult>('tts_narrate', { documentId, kind }),

  probeDocumentEngine: () => invoke<boolean>('probe_document_engine'),
};

/** Subscribe to model-download progress events (no-op in browser preview). */
export async function onModelDownload(
  handler: (ev: ModelDownloadEvent) => void,
): Promise<() => void> {
  if (!isNative) return () => {};
  const unlisten = await listen<ModelDownloadEvent>('ai://model-download', (e) =>
    handler(e.payload),
  );
  return unlisten;
}

/** Subscribe to streaming ask deltas (no-op in browser preview). */
export async function onAskDelta(
  handler: (text: string) => void,
): Promise<() => void> {
  if (!isNative) return () => {};
  const unlisten = await listen<{ text: string }>('ai://ask-delta', (e) =>
    handler(e.payload.text),
  );
  return unlisten;
}

/** Subscribe to export progress events (no-op in browser preview). */
export async function onExportProgress(
  handler: (ev: ExportProgressEvent) => void,
): Promise<() => void> {
  if (!isNative) return () => {};
  const unlisten = await listen<ExportProgressEvent>('exports://progress', (e) =>
    handler(e.payload),
  );
  return unlisten;
}

// ---------------------------------------------------------------------------
// Browser mock backend (non-native preview)
// ---------------------------------------------------------------------------

interface MockProject {
  project: Project;
  documentCount: number;
}

const MOCK_PROJECTS: MockProject[] = [
  {
    project: {
      id: 'mock-1',
      name: 'Thesis — Climate Adaptation',
      description: 'Doctoral research on coastal adaptation strategies and policy evidence.',
      createdAt: '2026-09-21T09:12:00Z',
      updatedAt: '2026-09-29T08:40:00Z',
    },
    documentCount: 24,
  },
  {
    project: {
      id: 'mock-2',
      name: 'Lit Review — Sleep & Memory',
      description: 'Systematic review of sleep-dependent memory consolidation studies.',
      createdAt: '2026-09-24T14:03:00Z',
      updatedAt: '2026-09-28T19:25:00Z',
    },
    documentCount: 8,
  },
  {
    project: {
      id: 'mock-3',
      name: 'Coursework — Research Methods',
      description: null,
      createdAt: '2026-09-29T07:55:00Z',
      updatedAt: '2026-09-29T07:55:00Z',
    },
    documentCount: 0,
  },
];

let mockDataDirectory = 'ResearchAIData (demo mode — browser preview)';

const MOCK_DOCS: DocumentSummary[] = [];

let MOCK_SETTINGS: AppSettings = {
  theme: 'system',
  dataDirectory: mockDataDirectory,
  defaultImportMode: 'managed-copy',
  ocrEnabled: true,
  maxConcurrentJobs: 2,
  aiEnabled: false,
};

// -- Local AI mock state (Phase 3) ------------------------------------------

const MOCK_MODELS: LocalModel[] = [
  {
    id: 'mock-model-1',
    fileName: 'Qwen3-0.6B-Q4_K_M.gguf',
    filePath: 'ResearchAIData/models/Qwen3-0.6B-Q4_K_M.gguf',
    sizeBytes: 479_820_000,
    sha256: 'mock-sha256',
    parameters: '0.6B',
    quantization: 'Q4_K_M',
    contextTokens: 40960,
    status: 'available',
    statusDetail: null,
    source: 'imported',
    addedAt: '2026-09-29T10:00:00Z',
    lastUsedAt: null,
  },
];

let MOCK_AI: AiSettings = {
  activeModelId: 'mock-model-1',
  llamaServerPath: '/opt/homebrew/bin/llama-server',
  llamaServerArgs: '',
  contextSize: 4096,
  maxTokens: 1024,
  temperature: 0.2,
  gpuLayers: 0,
  threads: 0,
  idleUnloadMinutes: 10,
};

let MOCK_RUNTIME: RuntimeStatus = {
  state: { state: 'idle' },
  modelFile: null,
  pid: 4242,
  idleSeconds: null,
};

interface MockAnalysisRecord {
  summary: AnalysisSummary;
  response: AnalysisResponse;
}
const MOCK_ANALYSES: MockAnalysisRecord[] = [];

// -- Evidence matrix mock state (Phase 4) ------------------------------------

interface MockEvidenceRecord {
  id: string;
  response: EvidenceResponse;
}
const MOCK_EVIDENCE: MockEvidenceRecord[] = [];

// -- Export mocks (Phase 6) --------------------------------------------------

const MOCK_EXPORTS: ExportFile[] = [];

// -- Speech-to-text mocks (Phase 7) -------------------------------------------

let MOCK_STT_SETTINGS: SttSettings = {
  whisperCliPath: '/opt/whisper.cpp/build/bin/whisper-cli',
  whisperModelPath: '/opt/whisper.cpp/models/ggml-base.bin',
  language: 'auto',
  convertWithFfmpeg: true,
};

// -- Text-to-speech mocks (Phase 8) -------------------------------------------

let MOCK_TTS_SETTINGS: TtsSettings = {
  provider: 'piper',
  piperPath: '/opt/homebrew/bin/piper',
  voiceModelPath: '/opt/piper/voices/en_US-amy-medium.onnx',
  speed: 1.0,
  macosVoice: 'Samantha',
  mp3Enabled: true,
};

function mockNarration(fileName: string, words: number, engine: string): NarrationResult {
  return {
    audioPath: `ResearchAIData/exports/tts-${fileName}-${engine}.mp3`,
    format: 'mp3',
    bytes: words * 22,
    durationMs: Math.round((words / 150) * 60_000),
    words,
    engine,
  };
}

function mockEvidenceRow(
  doc: DocumentSummary,
  question: string,
  i: number,
): EvidenceResponse['table']['rows'][number] {
  const hit = doc.indexingStatus === 'ready';
  return {
    documentId: doc.id,
    documentName: doc.fileName,
    strength: hit ? (i % 2 === 0 ? 'direct' : 'indirect') : 'none',
    score: hit ? 1 / (60 + i) : null,
    excerpts: hit
      ? [
          {
            chunkId: `mock-chunk-${doc.id}-${i}`,
            pageNumber: 1 + i,
            sectionHeading: 'Preview section',
            text: `Preview excerpt ${i + 1} matching “${question}” — the desktop app retrieves this document's best passages with page references.`,
            matchedBy: i % 2 === 0 ? ['keyword', 'vector'] : ['keyword'],
            vecDistance: 0.18 + i * 0.04,
            ftsRank: -2.1 - i,
          },
        ]
      : [],
  };
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function mockInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  await delay(60); // simulate IPC latency so loading states are visible
  switch (cmd) {
    case 'list_projects': {
      const projects: Project[] = MOCK_PROJECTS.map((p) => p.project);
      return clone(projects) as T;
    }
    case 'create_project': {
      const name = String(args?.['name'] ?? '').trim();
      const description = (args?.['description'] as string | null) ?? null;
      if (!name) {
        throw new Error('Project name must not be empty.');
      }
      const now = new Date().toISOString();
      const created: Project = {
        id: `mock-${Date.now()}`,
        name,
        description,
        createdAt: now,
        updatedAt: now,
      };
      MOCK_PROJECTS.push({ project: created, documentCount: 0 });
      return clone(created) as T;
    }
    case 'delete_project': {
      const id = String(args?.['id'] ?? '');
      const idx = MOCK_PROJECTS.findIndex((p) => p.project.id === id);
      if (idx >= 0) MOCK_PROJECTS.splice(idx, 1);
      return null as T;
    }
    case 'get_settings':
      return clone(MOCK_SETTINGS) as T;
    case 'set_theme': {
      MOCK_SETTINGS = {
        ...MOCK_SETTINGS,
        theme: args?.['theme'] as AppSettings['theme'],
      };
      return clone(MOCK_SETTINGS) as T;
    }
    case 'set_ai_enabled': {
      MOCK_SETTINGS = {
        ...MOCK_SETTINGS,
        aiEnabled: Boolean(args?.['enabled']),
      };
      return clone(MOCK_SETTINGS) as T;
    }
    case 'ai_pick_model_file': {
      const picked = window.prompt(
        'Browser preview: type a GGUF file path to simulate selection (cancel to abort).',
        '/Users/you/Downloads/Qwen3-0.6B-Q4_K_M.gguf',
      );
      return (picked ? [picked] : null) as unknown as T;
    }
    case 'get_system_info': {
      const nav = navigator as Navigator & { deviceMemory?: number };
      return {
        osName: 'Browser (preview)',
        osVersion: navigator.userAgent,
        arch: navigator.platform || 'unknown',
        cpu: { name: 'unknown (browser preview)', cores: navigator.hardwareConcurrency || 4 },
        totalMemoryMb: (nav.deviceMemory ?? 8) * 1024,
        availableMemoryMb: (nav.deviceMemory ?? 8) * 1024,
        profile: (nav.deviceMemory ?? 8) >= 16 ? 'standard' : 'light',
      } as T;
    }
    case 'run_diagnostics':
      return {
        system: await mockInvoke<SystemInfo>('get_system_info'),
        checks: [
          { id: 'data-directory', label: 'Data directory', status: 'pass', detail: mockDataDirectory },
          { id: 'database', label: 'SQLite database', status: 'pass', detail: 'Mock store (browser preview)' },
          { id: 'engine', label: 'Document engine (Python sidecar)', status: 'warn', detail: 'Not reachable from browser preview' },
          { id: 'native-shell', label: 'Native shell', status: 'warn', detail: 'Running in browser preview — Tauri IPC unavailable' },
        ],
        dataDirectory: mockDataDirectory,
        databaseOk: true,
        documentEngineOk: false,
        generatedAt: new Date().toISOString(),
      } as T;
    case 'pick_folder':
    case 'pick_documents': {
      const picked = window.prompt(
        'Browser preview: type a file path to simulate selection (cancel to abort).',
        '/Users/you/Downloads/paper.pdf',
      );
      return (picked ? [picked] : null) as T;
    }
    case 'list_documents': {
      const projectId = String(args?.['projectId'] ?? '');
      const docs = MOCK_DOCS.filter((d) => d.projectId === projectId);
      return clone(docs) as T;
    }
    case 'import_documents': {
      const projectId = String(args?.['projectId'] ?? '');
      const paths = (args?.['paths'] as string[] | undefined) ?? [];
      let imported = 0;
      for (const p of paths) {
        const fileName = p.split('/').pop() ?? p;
        const ext = fileName.split('.').pop()?.toLowerCase() ?? '';
        if (!['pdf', 'docx', 'txt', 'md', 'html', 'htm'].includes(ext)) {
          continue; // counted as an error in the real backend
        }
        const doc: { -readonly [K in keyof DocumentSummary]: DocumentSummary[K] } = {
          id: `mock-doc-${Date.now()}-${imported}`,
          projectId,
          fileName,
          originalPath: p,
          managedPath: null,
          documentType: ext,
          checksum: `mock-${Math.random().toString(36).slice(2)}`,
          title: null,
          authors: null,
          year: null,
          doi: null,
          journal: null,
          volume: null,
          issue: null,
          pages: null,
          publisher: null,
          url: null,
          refType: 'article',
          indexingStatus: 'parsing',
          statusDetail: null,
          pageCount: null,
          language: null,
          importedAt: new Date().toISOString(),
          chunkCount: 0,
        };
        MOCK_DOCS.push(doc);
        imported += 1;
        // Simulate the async pipeline: parsing → ready shortly after.
        window.setTimeout(() => {
          doc.indexingStatus = 'ready';
          doc.pageCount = 8;
          doc.chunkCount = 14;
        }, 2500);
      }
      return { imported, duplicates: 0, errors: [] } as T;
    }
    case 'retry_document':
    case 'delete_document': {
      const id = String(args?.['documentId'] ?? args?.['id'] ?? '');
      const idx = MOCK_DOCS.findIndex((d) => d.id === id);
      if (idx >= 0 && cmd === 'delete_document') MOCK_DOCS.splice(idx, 1);
      return null as T;
    }
    case 'get_document_text':
      return 'This is a preview mock. Parsed document text appears in the desktop app.'.replaceAll(
        '. ',
        '.\n\n',
      ) as T;
    // -- Citations & bibliography (Phase 5) ----------------------------------
    case 'update_document_bibliography': {
      const id = String(args?.['documentId'] ?? '');
      const update = (args?.['update'] ?? {}) as BibliographyUpdate;
      const doc = MOCK_DOCS.find((d) => d.id === id);
      if (!doc) throw new Error('Document not found.');
      const mutable = doc as { -readonly [K in keyof DocumentSummary]: DocumentSummary[K] };
      if (update.title !== undefined) mutable.title = update.title;
      if (update.authors !== undefined) mutable.authors = update.authors;
      if (update.year !== undefined) mutable.year = update.year;
      if (update.doi !== undefined) mutable.doi = update.doi;
      if (update.journal !== undefined) mutable.journal = update.journal;
      if (update.volume !== undefined) mutable.volume = update.volume;
      if (update.issue !== undefined) mutable.issue = update.issue;
      if (update.pages !== undefined) mutable.pages = update.pages;
      if (update.publisher !== undefined) mutable.publisher = update.publisher;
      if (update.url !== undefined) mutable.url = update.url;
      if (update.refType) mutable.refType = update.refType;
      return null as T;
    }
    case 'bibliography_list': {
      const style = (String(args?.['style'] ?? 'apa') || 'apa') as CitationStyle;
      const refs: FormattedReference[] = MOCK_DOCS.filter(
        (d) => d.indexingStatus === 'ready',
      ).map((d) => {
        const authors = (d.authors ?? 'Unknown Author').split(';').map((s) => s.trim());
        const year = d.year ?? 'n.d.';
        const title = d.title ?? d.fileName;
        const reference =
          style === 'apa'
            ? `${authors.join(', & ')} (${year}). ${title}.`
            : style === 'harvard'
              ? `${authors.join(' and ')} (${year}) ‘${title}’.`
              : `${authors.join(', ')}. “${title}.”`;
        return {
          documentId: d.id,
          style,
          reference: `${reference} (preview)`,
          inText: `(${authors[0]}, ${year})`,
          incomplete: d.title == null || d.year == null,
        };
      });
      return refs as T;
    }
    case 'search_library': {
      const q = String(args?.['query'] ?? '');
      const hits = MOCK_DOCS.filter((d) => d.indexingStatus === 'ready').map((d, i) => ({
        chunkId: `mock-chunk-${i}`,
        documentId: d.id,
        documentName: d.fileName,
        pageNumber: d.pageCount ? Math.min(i + 1, d.pageCount) : null,
        sectionHeading: 'Preview section',
        text: `Preview hit for “${q}” — the desktop app returns real chunks fused from keyword and vector channels with scores and citations.`,
        startOffset: null,
        endOffset: null,
        score: 1 / (60 + i),
        matchedBy: i % 2 === 0 ? ['keyword', 'vector'] : ['vector'],
        ftsRank: i % 2 === 0 ? -2.4 - i : null,
        vecDistance: 0.2 + i * 0.05,
      }));
      return {
        hits,
        trace: {
          query: q,
          keywordCandidates: hits.filter((h) => h.matchedBy.includes('keyword')).length,
          vectorCandidates: hits.filter((h) => h.matchedBy.includes('vector')).length,
          fused: hits.length,
          returned: hits.length,
          embeddingEngine: 'fastembed (preview)',
          embeddingCoverage: `${MOCK_DOCS.length} doc(s) embedded (preview)`,
          warnings: [],
        },
      } as T;
    }
    case 'retrieval_status':
      return {
        embeddingEngine: 'fastembed (preview)',
        totalChunks: MOCK_DOCS.length * 14,
        embeddedChunks: MOCK_DOCS.filter((d) => d.indexingStatus === 'ready').length * 14,
      } as T;
    // -- Local AI (Phase 3) --------------------------------------------------
    case 'ai_list_models':
      return clone(MOCK_MODELS) as T;
    case 'ai_add_model': {
      const path = String(args?.['path'] ?? '');
      const fileName = path.split('/').pop() ?? 'model.gguf';
      if (!fileName.toLowerCase().endsWith('.gguf')) {
        throw new Error('Not a GGUF model file — local models must be .gguf (llama.cpp format).');
      }
      const quant = fileName.match(/Q\d[_A-Z0-9]*|IQ\d[_A-Z0-9]*/i)?.[0] ?? null;
      const model: LocalModel = {
        id: `mock-model-${Date.now()}`,
        fileName,
        filePath: `ResearchAIData/models/${fileName}`,
        sizeBytes: 500_000_000,
        sha256: `mock-${Math.random().toString(36).slice(2)}`,
        parameters: '0.6B',
        quantization: quant,
        contextTokens: null,
        status: 'available',
        statusDetail: null,
        source: 'imported',
        addedAt: new Date().toISOString(),
        lastUsedAt: null,
      };
      MOCK_MODELS.unshift(model);
      return clone(model) as T;
    }
    case 'ai_download_model': {
      const url = String(args?.['url'] ?? '');
      const fileName = url.split('?')[0].split('/').pop() ?? 'model.gguf';
      window.setTimeout(() => {
        MOCK_MODELS.unshift({
          id: `mock-model-${Date.now()}`,
          fileName,
          filePath: `ResearchAIData/models/${fileName}`,
          sizeBytes: 1_200_000_000,
          sha256: `mock-${Math.random().toString(36).slice(2)}`,
          parameters: '1.7B',
          quantization: 'Q4_K_M',
          contextTokens: null,
          status: 'available',
          statusDetail: null,
          source: 'downloaded',
          addedAt: new Date().toISOString(),
          lastUsedAt: null,
        });
      }, 1500);
      return 1 as T; // job id — progress events are native-only
    }
    case 'ai_cancel_download':
      return true as T;
    case 'ai_delete_model': {
      const id = String(args?.['modelId'] ?? '');
      const idx = MOCK_MODELS.findIndex((m) => m.id === id);
      if (idx >= 0) MOCK_MODELS.splice(idx, 1);
      return null as T;
    }
    case 'ai_get_settings':
      return clone(MOCK_AI) as T;
    case 'ai_save_settings':
      MOCK_AI = { ...MOCK_AI, ...(args?.['settings'] as AiSettings) };
      return clone(MOCK_AI) as T;
    case 'ai_runtime_status':
      return clone(MOCK_RUNTIME) as T;
    case 'ai_load_model': {
      if (!MOCK_AI.activeModelId) {
        throw new Error('No local model is selected. Add a GGUF model and choose one.');
      }
      const model = MOCK_MODELS.find((m) => m.id === MOCK_AI.activeModelId);
      if (!model) throw new Error('The selected model is not in the library.');
      MOCK_RUNTIME = {
        ...MOCK_RUNTIME,
        state: { state: 'loading' },
        modelFile: model.fileName,
      };
      window.setTimeout(() => {
        MOCK_RUNTIME = { ...MOCK_RUNTIME, state: { state: 'ready', port: 8941 } };
      }, 1200);
      return clone(MOCK_RUNTIME) as T;
    }
    case 'ai_unload_model':
      MOCK_RUNTIME = { ...MOCK_RUNTIME, state: { state: 'unloaded' }, modelFile: null };
      return clone(MOCK_RUNTIME) as T;
    case 'ai_ask': {
      if (!MOCK_SETTINGS.aiEnabled) {
        throw new Error('AI is disabled (No-AI mode). Enable it in Settings to use Ask features.');
      }
      const mode = String(args?.['mode'] ?? 'chat');
      const question = String(args?.['question'] ?? '');
      const readyDocs = MOCK_DOCS.filter((d) => d.indexingStatus === 'ready');
      if (readyDocs.length === 0) {
        return {
          analysisId: null,
          answer:
            'No indexed sources matched this question. Try Search to find the right documents, then ask again with a wider scope.',
          evidence: [],
          trace: {
            mode,
            engine: 'llama.cpp (preview)',
            model: MOCK_RUNTIME.modelFile,
            scopeDocuments: 0,
            evidenceCount: 0,
            evidenceChars: 0,
            citationsUsed: [],
            promptTokens: null,
            completionTokens: null,
            finishReason: null,
            durationMs: 60,
            embeddingCoverage: `${MOCK_DOCS.length} doc(s) embedded (preview)`,
            warnings: ['Preview mock — no documents imported yet.'],
          },
        } as T;
      }
      const evidence: AnalysisEvidence[] = readyDocs.slice(0, 3).map((d, i) => ({
        chunkId: `mock-chunk-${i}`,
        documentId: d.id,
        documentName: d.fileName,
        pageNumber: 1 + i,
        sectionHeading: 'Preview section',
        text: `Preview evidence ${i + 1} for “${question}” — the desktop app sends numbered excerpts from your library to the local model.`,
        startOffset: null,
        endOffset: null,
      }));
      const response: AnalysisResponse = {
        analysisId: `mock-analysis-${Date.now()}`,
        answer: `Preview answer (mode: ${mode}) for “${question}”. The local model answers strictly from the numbered evidence: the key claim is in [1], with supporting detail in [2]. In the packaged app every sentence carries citations you can click to open the exact page.`,
        evidence,
        trace: {
          mode,
          engine: 'llama.cpp (preview)',
          model: MOCK_RUNTIME.modelFile,
          scopeDocuments: 0,
          evidenceCount: evidence.length,
          evidenceChars: evidence.reduce((a, e) => a + e.text.length, 0),
          citationsUsed: [1, 2],
          promptTokens: 512,
          completionTokens: 96,
          finishReason: 'stop',
          durationMs: 1250,
          embeddingCoverage: `${MOCK_DOCS.length} doc(s) embedded (preview)`,
          warnings: [],
        },
      };
      MOCK_ANALYSES.unshift({
        summary: {
          id: response.analysisId!,
          analysisType: mode,
          question,
          createdAt: new Date().toISOString(),
          modelId: MOCK_AI.activeModelId,
        },
        response,
      });
      return clone(response) as T;
    }
    case 'ai_list_analyses':
      return clone(MOCK_ANALYSES.map((a) => a.summary)) as T;
    case 'ai_get_analysis': {
      const id = String(args?.['analysisId'] ?? '');
      const rec = MOCK_ANALYSES.find((a) => a.summary.id === id);
      if (!rec) throw new Error('Analysis not found.');
      return clone(rec.response) as T;
    }
    // -- Evidence matrices (Phase 4) -----------------------------------------
    case 'evidence_build': {
      const question = String(args?.['question'] ?? '');
      const withAi = Boolean(args?.['withAi']);
      if (!question.trim()) throw new Error('The question must not be empty.');
      const docs = MOCK_DOCS.filter(
        (d) => d.indexingStatus === 'ready',
      ).slice(0, 12);
      const rows = docs.map((d, i) => mockEvidenceRow(d, question, i));
      const matched = rows.filter((r) => r.strength !== 'none');
      const findings = withAi
        ? matched.map((r, i) => ({
            documentId: r.documentId,
            documentName: r.documentName,
            text: `Preview findings [${i + 1}]: this document addresses the question in its matching passages and states its position with cited support.`,
            citationsUsed: [1],
          }))
        : [];
      const synthesis = withAi
        ? `Preview synthesis. AGREEMENTS: the matched documents [1][2] describe the topic consistently. DISAGREEMENTS: none visible in the excerpts. GAPS: the excerpts do not cover methodological detail.`
        : null;
      return {
        tableId: null,
        modelId: withAi ? 'mock-model-1' : null,
        table: { question, rows, findings, synthesis, synthesisCitations: withAi ? [1, 2] : [] },
        trace: {
          mode: withAi && matched.length > 0 ? 'ai' : 'deterministic',
          engine: withAi && matched.length > 0 ? 'llama.cpp (preview)' : 'deterministic',
          model: withAi ? MOCK_RUNTIME.modelFile : null,
          scopeDocuments: docs.length,
          documentsMatched: matched.length,
          totalExcerpts: rows.reduce((a, r) => a + r.excerpts.length, 0),
          strengthCounts: rows.reduce<Record<string, number>>((acc, r) => {
            acc[r.strength] = (acc[r.strength] ?? 0) + 1;
            return acc;
          }, {}),
          retrievalWarnings: [],
          warnings: docs.length === 0 ? ['No ready documents in scope — import documents first.'] : [],
          durationMs: 140,
        },
      } as T;
    }
    case 'evidence_save': {
      const id = `mock-evidence-${Date.now()}`;
      const table = args?.['table'] as EvidenceTable;
      const trace = args?.['trace'] as EvidenceTrace;
      MOCK_EVIDENCE.unshift({
        id,
        response: { tableId: id, modelId: (args?.['modelId'] as string | null) ?? null, table, trace },
      });
      return id as T;
    }
    case 'evidence_list':
      return clone(
        MOCK_EVIDENCE.map((r) => ({
          id: r.id,
          question: r.response.table.question,
          createdAt: new Date().toISOString(),
          hasAi: r.response.modelId != null,
          modelId: r.response.modelId,
        })),
      ) as T;
    case 'evidence_get': {
      const id = String(args?.['tableId'] ?? '');
      const rec = MOCK_EVIDENCE.find((r) => r.id === id);
      if (!rec) throw new Error('Evidence table not found.');
      return clone(rec.response) as T;
    }
    case 'evidence_delete': {
      const id = String(args?.['tableId'] ?? '');
      const idx = MOCK_EVIDENCE.findIndex((r) => r.id === id);
      if (idx >= 0) MOCK_EVIDENCE.splice(idx, 1);
      return null as T;
    }
    // -- Academic exports (Phase 6) ------------------------------------------
    case 'export_capabilities':
      return {
        formats: ['markdown', 'docx', 'pdf', 'bibtex', 'ris'],
        kinds: [
          { kind: 'analysis', formats: ['markdown', 'docx', 'pdf'] },
          { kind: 'evidence_table', formats: ['markdown', 'docx', 'pdf', 'bibtex', 'ris'] },
          { kind: 'bibliography', formats: ['markdown', 'docx', 'pdf', 'bibtex', 'ris'] },
        ],
      } as T;
    case 'export_document': {
      const kind = String(args?.['kind'] ?? '');
      const format = String(args?.['format'] ?? 'markdown');
      const name = `mock-${kind}-${new Date().toISOString().slice(0, 19).replaceAll(':', '-')}.${format === 'markdown' ? 'md' : format}`;
      const file: ExportFile = {
        name,
        path: `ResearchAIData/exports/${name}`,
        sizeBytes: 1024 + Math.floor(Math.random() * 4000),
      };
      MOCK_EXPORTS.unshift(file);
      return {
        path: file.path,
        bytes: file.sizeBytes,
        kind,
        format,
      } as T;
    }
    case 'export_bibliography': {
      const format = String(args?.['format'] ?? 'bibtex');
      const name = `mock-bibliography.${format === 'bibtex' ? 'bib' : 'ris'}`;
      const file: ExportFile = {
        name,
        path: `ResearchAIData/exports/${name}`,
        sizeBytes: 800,
      };
      MOCK_EXPORTS.unshift(file);
      return { path: file.path, bytes: 800, kind: 'bibliography', format } as T;
    }
    case 'list_exports':
      return clone(MOCK_EXPORTS) as T;
    case 'exports_stats':
      return {
        files: MOCK_EXPORTS.length,
        totalBytes: MOCK_EXPORTS.reduce((sum, f) => sum + f.sizeBytes, 0),
      } as T;
    case 'delete_export': {
      const p = String(args?.['path'] ?? '');
      const idx = MOCK_EXPORTS.findIndex((f) => f.path === p);
      if (idx < 0) throw new Error('The export file no longer exists.');
      MOCK_EXPORTS.splice(idx, 1);
      return {
        files: MOCK_EXPORTS.length,
        totalBytes: MOCK_EXPORTS.reduce((sum, f) => sum + f.sizeBytes, 0),
      } as T;
    }
    case 'reveal_path':
      return String(args?.['path'] ?? '') as T;
    // -- Speech-to-text (Phase 7) ---------------------------------------------
    case 'stt_check': {
      void args;
      const s = MOCK_STT_SETTINGS;
      return {
        configured: Boolean(s.whisperCliPath && s.whisperModelPath),
        cliFound: true,
        modelFound: true,
        ffmpegFound: true,
        ...s,
      } as T;
    }
    case 'stt_get_settings':
      return clone(MOCK_STT_SETTINGS) as T;
    case 'stt_save_settings': {
      MOCK_STT_SETTINGS = clone(args?.['settings'] as SttSettings);
      return clone(MOCK_STT_SETTINGS) as T;
    }
    case 'stt_transcribe': {
      const projectId = String(args?.['projectId'] ?? '');
      const audioPath = String(args?.['audioPath'] ?? '');
      const fileName = audioPath.split('/').pop() || 'lecture.wav';
      const doc: { -readonly [K in keyof DocumentSummary]: DocumentSummary[K] } = {
        id: `mock-transcript-${Date.now()}`,
        projectId,
        fileName,
        originalPath: audioPath,
        managedPath: null,
        documentType: 'transcript',
        checksum: `mock-${Math.random().toString(36).slice(2)}`,
        title: `Transcript: ${fileName.replace(/\.[^.]+$/, '')}`,
        authors: null,
        year: null,
        doi: null,
        journal: null,
        volume: null,
        issue: null,
        pages: null,
        publisher: null,
        url: null,
        refType: 'transcript',
        indexingStatus: 'ready',
        statusDetail: 'Local transcription · 15 min · en',
        pageCount: null,
        language: 'en',
        importedAt: new Date().toISOString(),
        chunkCount: 2,
      };
      MOCK_DOCS.push(doc);
      return {
        documentId: doc.id,
        segments: 2,
        durationMs: 912_000,
        language: 'en',
        detail: '',
      } as T;
    }
    // -- Text-to-speech (Phase 8) ---------------------------------------------
    case 'tts_check': {
      void args;
      const s = MOCK_TTS_SETTINGS;
      return {
        provider: s.provider,
        binaryFound: true,
        modelFound: true,
        ffmpegFound: true,
        ready: true,
        outputDir: 'ResearchAIData/exports',
        settings: clone(s),
      } as T;
    }
    case 'tts_get_settings':
      return clone(MOCK_TTS_SETTINGS) as T;
    case 'tts_save_settings': {
      MOCK_TTS_SETTINGS = clone(args?.['settings'] as TtsSettings);
      return clone(MOCK_TTS_SETTINGS) as T;
    }
    case 'tts_speak_document': {
      const doc = MOCK_DOCS[0];
      const name = doc ? doc.fileName.replace(/\.[^.]+$/, '') : 'document';
      return mockNarration(name, 2400, 'read_aloud') as T;
    }
    case 'tts_narrate': {
      const kind = String(args?.['kind'] ?? 'summary_5');
      if (!MOCK_SETTINGS.aiEnabled) {
        throw new Error(
          'AI is disabled (No-AI mode). Summaries and podcast narration need the local model — read-aloud still works.',
        );
      }
      const doc = MOCK_DOCS[0];
      const name = doc ? doc.fileName.replace(/\.[^.]+$/, '') : 'document';
      const words = kind === 'podcast' ? 1200 : kind === 'summary_20' ? 2600 : kind === 'summary_10' ? 1300 : 650;
      return mockNarration(name, words, kind === 'podcast' ? 'llama.cpp' : 'llama.cpp') as T;
    }
    default:
      throw new Error(`Browser preview has no mock for command "${cmd}".`);
  }
}

// Re-exported for potential typed error mapping later (spec §42).
export type { Result };
