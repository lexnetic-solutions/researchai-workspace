import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  AnalysisMode,
  AnalysisResponse,
  AnalysisSummary,
  DocumentSummary,
  RuntimeStatus,
} from '@researchai/shared-types';
import { backend, onAskDelta, onModelDownload } from '../backend/client';
import { CitedText } from '../components/CitedText';
import { EmptyState } from '../components/EmptyState';
import { useStore } from '../state/store';

const MODES: readonly { id: AnalysisMode; name: string; desc: string }[] = [
  { id: 'chat', name: 'Chat', desc: 'Plain-language questions about your selection.' },
  { id: 'research', name: 'Research', desc: 'Strict source-grounded answers with citations and uncertainty flags.' },
  { id: 'quick_read', name: 'Quick Read', desc: 'Title, question, five findings, method, conclusion, limitations.' },
  { id: 'deep_analysis', name: 'Deep Analysis', desc: 'WHAT / WHY / WHO / HOW / EVIDENCE / STRENGTHS / LIMITATIONS / GAPS.' },
  { id: 'critical', name: 'Critical', desc: 'Methodological challenge pass over one paper.' },
] as const;

const MODE_LABEL: Record<string, string> = Object.fromEntries(
  MODES.map((m) => [m.id, m.name]),
);

type Scope = 'project' | 'document';

function loadStateLabel(state: RuntimeStatus['state']): string {
  switch (state.state) {
    case 'idle':
      return 'Model not loaded';
    case 'loading':
      return 'Loading model…';
    case 'ready':
      return `Ready (port ${state.port})`;
    case 'failed':
      return 'Load failed';
    case 'unloaded':
      return 'Unloaded';
  }
}


export function ResearchView() {
  const { activeProject, settings, pushToast, native } = useStore();
  const [mode, setMode] = useState<AnalysisMode>('research');
  const [scope, setScope] = useState<Scope>('project');
  const [scopeDocs, setScopeDocs] = useState<DocumentSummary[]>([]);
  const [question, setQuestion] = useState('');
  const [asking, setAsking] = useState(false);
  const [streamText, setStreamText] = useState('');
  const [result, setResult] = useState<AnalysisResponse | null>(null);
  const [showDebug, setShowDebug] = useState(false);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [modelLoading, setModelLoading] = useState(false);
  const [history, setHistory] = useState<AnalysisSummary[]>([]);
  const debounce = useRef<number | null>(null);

  const aiEnabled = settings?.aiEnabled ?? false;

  const loadHistory = useCallback(async () => {
    if (!activeProject) return;
    try {
      setHistory(await backend.aiListAnalyses(activeProject.id, 20));
    } catch {
      /* history is non-critical */
    }
  }, [activeProject]);

  const refreshRuntime = useCallback(async () => {
    try {
      setRuntime(await backend.aiRuntimeStatus());
    } catch {
      setRuntime(null);
    }
  }, []);

  useEffect(() => {
    if (!activeProject) return;
    void backend
      .listDocuments(activeProject.id)
      .then(setScopeDocs)
      .catch(() => setScopeDocs([]));
    void loadHistory();
    void refreshRuntime();
    // Streamed ask deltas render live under the asking notice; the
    // subscription persists for the view's lifetime.
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onAskDelta((text) => {
      setStreamText((prev) => prev + text);
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    void onModelDownload(() => {}).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [activeProject, loadHistory, refreshRuntime]);

  const readyDocs = useMemo(
    () => scopeDocs.filter((d) => d.indexingStatus === 'ready'),
    [scopeDocs],
  );

  const scopeIds = useMemo(() => {
    if (scope === 'document') return readyDocs.slice(0, 1).map((d) => d.id);
    return readyDocs.map((d) => d.id);
  }, [scope, readyDocs]);

  const runtimeState = runtime?.state.state ?? 'idle';

  async function ask(q: string) {
    if (!activeProject || !q.trim() || asking) return;
    if (scopeIds.length === 0) {
      pushToast('error', 'No ready documents in scope — import documents or wait for indexing.');
      return;
    }
    setAsking(true);
    setStreamText('');
    try {
      // Stream path: deltas arrive via ai://ask-delta and render live while
      // the awaited call resolves with the full persisted response.
      const res = await backend.aiAskStream(activeProject.id, scopeIds, mode, q.trim());
      setResult(res);
      setStreamText('');
      void refreshRuntime();
      void loadHistory();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
      void refreshRuntime();
    } finally {
      setAsking(false);
      setStreamText('');
    }
  }

  function onQuestionInput(value: string) {
    setQuestion(value);
    if (debounce.current) window.clearTimeout(debounce.current);
  }

  async function loadModel() {
    setModelLoading(true);
    try {
      const status = await backend.aiLoadModel();
      setRuntime(status);
      if (status.state.state === 'ready') {
        pushToast('success', 'Local model is ready.');
      } else if (status.state.state === 'failed') {
        pushToast('error', status.state.detail);
      }
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
      void refreshRuntime();
    } finally {
      setModelLoading(false);
    }
  }

  async function unloadModel() {
    try {
      setRuntime(await backend.aiUnloadModel());
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function openHistory(id: string) {
    try {
      setResult(await backend.aiGetAnalysis(id));
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  if (!activeProject) {
    return (
      <section className="view">
        <header className="view-head">
          <h1>Research AI</h1>
          <p className="view-sub">Select a project to ask questions about it.</p>
        </header>
        <EmptyState title="No active project" hint="Create or select a project first." />
      </section>
    );
  }

  return (
    <section className="view">
      <header className="view-head">
        <h1>Research AI</h1>
        <p className="view-sub">
          Grounded question answering over your own library — local models only. Every answer
          cites the numbered evidence it was built from.
        </p>
      </header>

      {!aiEnabled && (
        <div className="card notice">
          <h2>AI is off (No-AI mode)</h2>
          <p>
            Enable AI in Settings to use Ask features. ResearchAI works fully without AI:
            import, read, search, annotate and export.
          </p>
          <p className="tiny muted">
            Cloud providers are opt-in later and never required (spec §35).
          </p>
        </div>
      )}

      {aiEnabled && (
        <>
          <div className="card runtime-banner">
            <div>
              <span className={`chip ${runtimeState === 'ready' ? 'pass' : runtimeState === 'failed' ? 'fail' : 'subtle'}`}>
                {loadStateLabel(runtime?.state ?? { state: 'idle' })}
              </span>
              {runtime?.modelFile && <span className="tiny muted"> {runtime.modelFile}</span>}
              {!native && <span className="tiny muted"> (preview mock)</span>}
            </div>
            <div className="field-row">
              <button
                type="button"
                className="btn ghost tiny-btn"
                disabled={modelLoading || runtimeState === 'ready' || runtimeState === 'loading'}
                onClick={() => void loadModel()}
              >
                {modelLoading ? 'Loading…' : 'Load model'}
              </button>
              <button
                type="button"
                className="btn ghost tiny-btn"
                disabled={runtimeState !== 'ready'}
                onClick={() => void unloadModel()}
              >
                Unload
              </button>
            </div>
          </div>

          <div className="mode-grid">
            {MODES.map((m) => (
              <button
                key={m.id}
                type="button"
                className={`mode-card ${mode === m.id ? 'active' : ''}`}
                onClick={() => setMode(m.id)}
              >
                <h3>{m.name}</h3>
                <p className="muted">{m.desc}</p>
              </button>
            ))}
          </div>

          <div className="card">
            <div className="search-row">
              <textarea
                className="search-input ask-input"
                placeholder={
                  scope === 'document'
                    ? 'Ask about the first document…'
                    : `Ask about ${readyDocs.length} indexed document(s)…`
                }
                rows={2}
                value={question}
                onChange={(e) => onQuestionInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    void ask(question);
                  }
                }}
                aria-label="Question"
              />
              <select
                className="search-scope"
                value={scope}
                onChange={(e) => {
                  setScope(e.target.value as Scope);
                  setResult(null);
                }}
                aria-label="Ask scope"
              >
                <option value="project">Active project</option>
                <option value="document">First document</option>
              </select>
              <button
                type="button"
                className="btn primary"
                disabled={asking || !question.trim()}
                onClick={() => void ask(question)}
              >
                {asking ? 'Asking…' : 'Ask'}
              </button>
            </div>
            <p className="tiny muted">
              The first Ask loads the local model and can take a minute; later answers are faster.
            </p>
          </div>

          {asking && (
            <div className="card notice">
              <p>Retrieving evidence and asking the local model…</p>
              {streamText && (
                <pre className="answer-stream tiny">{streamText}</pre>
              )}
            </div>
          )}

          {result && !asking && (
            <div className="card answer-card">
              <div className="hit-head">
                <span className="chip subtle">{MODE_LABEL[result.trace.mode] ?? result.trace.mode}</span>
                <span className="tiny muted">
                  {result.trace.engine}
                  {result.trace.model ? ` · ${result.trace.model}` : ''} ·{' '}
                  {result.trace.durationMs} ms
                  {result.trace.completionTokens != null
                    ? ` · ${result.trace.completionTokens} tokens`
                    : ''}
                </span>
                <button
                  type="button"
                  className="btn ghost tiny-btn"
                  onClick={() => setShowDebug((v) => !v)}
                >
                  {showDebug ? 'Hide' : 'Show'} debug
                </button>
              </div>

              <CitedText answer={result.answer} targetPrefix="evidence" />

              {showDebug && (
                <div className="debug-panel">
                  <h3>Analysis trace</h3>
                  <div className="debug-grid">
                    <div><span className="diag-label">Scope documents</span>{result.trace.scopeDocuments}</div>
                    <div><span className="diag-label">Evidence excerpts</span>{result.trace.evidenceCount} ({result.trace.evidenceChars} chars)</div>
                    <div><span className="diag-label">Citations used</span>{result.trace.citationsUsed.map((n) => `[${n}]`).join(' ') || 'none'}</div>
                    <div><span className="diag-label">Tokens (prompt/completion)</span>{result.trace.promptTokens ?? '–'} / {result.trace.completionTokens ?? '–'}</div>
                    <div><span className="diag-label">Finish reason</span>{result.trace.finishReason ?? '–'}</div>
                    <div><span className="diag-label">Index coverage</span>{result.trace.embeddingCoverage}</div>
                  </div>
                  {result.trace.warnings.length > 0 && (
                    <ul className="debug-warnings">
                      {result.trace.warnings.map((w) => (
                        <li key={w} className="tiny warn-text">{w}</li>
                      ))}
                    </ul>
                  )}
                </div>
              )}

              {result.evidence.length > 0 && (
                <>
                  <h3 className="evidence-title">Evidence</h3>
                  <ol className="evidence-list">
                    {result.evidence.map((e, i) => (
                      <li key={e.chunkId} id={`evidence-${i + 1}`} className="evidence-card">
                        <div className="hit-head">
                          <span className="chip">[{i + 1}]</span>
                          <span className="hit-doc">{e.documentName}</span>
                          {e.pageNumber != null && <span className="chip subtle">p. {e.pageNumber}</span>}
                          {e.sectionHeading && <span className="chip subtle">{e.sectionHeading}</span>}
                        </div>
                        <p className="hit-text">{e.text}</p>
                      </li>
                    ))}
                  </ol>
                  <p className="tiny muted">
                    Format these as citations in the Bibliography view (APA 7 · Harvard ·
                    Chicago) — metadata corrections in the Library reader are authoritative.
                  </p>
                </>
              )}
            </div>
          )}

          {history.length > 0 && (
            <div className="card">
              <h2>Recent analyses</h2>
              <ul className="history-list">
                {history.map((h) => (
                  <li key={h.id}>
                    <button type="button" className="history-item" onClick={() => void openHistory(h.id)}>
                      <span className="chip subtle">{MODE_LABEL[h.analysisType] ?? h.analysisType}</span>
                      <span className="history-question">{h.question ?? '(no question)'}</span>
                      <span className="tiny muted">{new Date(h.createdAt).toLocaleString()}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}
    </section>
  );
}
