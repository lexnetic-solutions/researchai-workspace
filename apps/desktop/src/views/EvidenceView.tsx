import { useCallback, useEffect, useState } from 'react';
import type {
  EvidenceResponse,
  EvidenceStrength,
  EvidenceSummary,
} from '@researchai/shared-types';
import { backend } from '../backend/client';
import { CitedText } from '../components/CitedText';
import { EmptyState } from '../components/EmptyState';
import { useStore } from '../state/store';

const STRENGTH_LABEL: Record<EvidenceStrength, string> = {
  direct: 'Direct',
  multiple: 'Multiple-source support',
  indirect: 'Indirect',
  none: 'No match',
};


export function EvidenceView() {
  const { activeProject, settings, pushToast } = useStore();
  const [question, setQuestion] = useState('');
  const [withAi, setWithAi] = useState(false);
  const [building, setBuilding] = useState(false);
  const [result, setResult] = useState<EvidenceResponse | null>(null);
  const [showDebug, setShowDebug] = useState(false);
  const [history, setHistory] = useState<EvidenceSummary[]>([]);
  const [saving, setSaving] = useState(false);

  const aiEnabled = settings?.aiEnabled ?? false;

  const loadHistory = useCallback(async () => {
    if (!activeProject) return;
    try {
      setHistory(await backend.evidenceList(activeProject.id, 20));
    } catch {
      /* history is non-critical */
    }
  }, [activeProject]);

  useEffect(() => {
    void loadHistory();
  }, [loadHistory]);

  async function build() {
    if (!activeProject || !question.trim() || building) return;
    setBuilding(true);
    try {
      const res = await backend.evidenceBuild(
        activeProject.id,
        [], // whole project; per-document scope arrives with the picker
        question.trim(),
        withAi && aiEnabled,
      );
      setResult(res);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setBuilding(false);
    }
  }

  async function save() {
    if (!activeProject || !result) return;
    setSaving(true);
    try {
      const id = await backend.evidenceSave(
        activeProject.id,
        result.table.question,
        [],
        result.table,
        result.trace,
        result.modelId,
      );
      pushToast('success', 'Evidence table saved.');
      void loadHistory();
      setResult({ ...result, tableId: id });
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  async function openSaved(id: string) {
    try {
      setResult(await backend.evidenceGet(id));
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function deleteSaved(id: string) {
    try {
      await backend.evidenceDelete(id);
      void loadHistory();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  if (!activeProject) {
    return (
      <section className="view">
        <header className="view-head">
          <h1>Evidence</h1>
          <p className="view-sub">Select a project to compare its documents.</p>
        </header>
        <EmptyState title="No active project" hint="Create or select a project first." />
      </section>
    );
  }

  const table = result?.table;

  return (
    <section className="view">
      <header className="view-head">
        <h1>Evidence</h1>
        <p className="view-sub">
          Cross-document evidence matrix — one row per document, with page-referenced
          excerpts and strength labels. The matrix itself is deterministic and works
          without AI.
        </p>
      </header>

      <div className="card">
        <div className="search-row">
          <input
            className="search-input"
            placeholder="e.g. Which documents cover delta sediment starvation?"
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void build();
            }}
            aria-label="Comparison question"
          />
          <button
            type="button"
            className="btn primary"
            disabled={building || !question.trim()}
            onClick={() => void build()}
          >
            {building ? 'Building…' : 'Build matrix'}
          </button>
        </div>
        <div className="field-row" style={{ marginTop: '0.5rem' }}>
          <label className="model-row tiny">
            <input
              type="checkbox"
              checked={withAi && aiEnabled}
              disabled={!aiEnabled}
              onChange={(e) => setWithAi(e.target.checked)}
            />
            <span>
              Include AI findings &amp; synthesis
              {!aiEnabled && ' (requires AI enabled in Settings)'}
            </span>
          </label>
        </div>
      </div>

      {building && (
        <div className="card notice">
          <p>Retrieving the best matching passages per document…</p>
        </div>
      )}

      {result && !building && table && (
        <div className="card matrix-card">
          <div className="hit-head">
            <h2 className="matrix-question">“{table.question}”</h2>
            <span className="tiny muted">
              {result.trace.documentsMatched}/{result.trace.scopeDocuments} documents
              matched · {result.trace.totalExcerpts} excerpts · {result.trace.mode}
            </span>
            <button
              type="button"
              className="btn ghost tiny-btn"
              onClick={() => setShowDebug((v) => !v)}
            >
              {showDebug ? 'Hide' : 'Show'} debug
            </button>
            <button
              type="button"
              className="btn tiny-btn"
              disabled={saving}
              onClick={() => void save()}
            >
              {saving ? 'Saving…' : 'Save'}
            </button>
          </div>

          {showDebug && (
            <div className="debug-panel">
              <h3>Evidence trace</h3>
              <div className="debug-grid">
                <div><span className="diag-label">Engine</span>{result.trace.engine}</div>
                <div><span className="diag-label">Excerpts</span>{result.trace.totalExcerpts}</div>
                <div>
                  <span className="diag-label">Strengths</span>
                  {Object.entries(result.trace.strengthCounts)
                    .map(([k, v]) => `${k}: ${v}`)
                    .join(' · ') || '–'}
                </div>
                <div><span className="diag-label">Duration</span>{result.trace.durationMs} ms</div>
              </div>
              {(result.trace.warnings.length > 0 || result.trace.retrievalWarnings.length > 0) && (
                <ul className="debug-warnings">
                  {[...result.trace.retrievalWarnings, ...result.trace.warnings].map((w) => (
                    <li key={w} className="tiny warn-text">{w}</li>
                  ))}
                </ul>
              )}
            </div>
          )}

          <table className="matrix-table">
            <thead>
              <tr>
                <th>Document</th>
                <th>Strength</th>
                <th>Excerpts</th>
              </tr>
            </thead>
            <tbody>
              {table.rows.map((row) => (
                <tr key={row.documentId} className={`matrix-strength-${row.strength}`}>
                  <td className="matrix-doc">{row.documentName}</td>
                  <td>
                    <span className={`chip strength-${row.strength}`}>
                      {STRENGTH_LABEL[row.strength]}
                    </span>
                    {row.score != null && (
                      <span className="tiny muted" title="Best fusion score"> {row.score.toFixed(4)}</span>
                    )}
                  </td>
                  <td>
                    {row.excerpts.length === 0 ? (
                      <span className="tiny muted">—</span>
                    ) : (
                      <ul className="excerpt-list">
                        {row.excerpts.map((e) => (
                          <li key={e.chunkId}>
                            {e.pageNumber != null && <span className="chip subtle">p. {e.pageNumber}</span>}
                            <span className="excerpt-text">{e.text}</span>
                            {e.matchedBy.map((m) => (
                              <span key={m} className={`chip channel-${m}`}>{m}</span>
                            ))}
                          </li>
                        ))}
                      </ul>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

          {table.findings.length > 0 && (
            <>
              <h3 className="evidence-title">AI findings per document</h3>
              <ol className="findings-list">
                {table.findings.map((f, i) => (
                  <li key={f.documentId} id={`matrix-item-${i + 1}`} className="evidence-card">
                    <div className="hit-head">
                      <span className="chip">[{i + 1}]</span>
                      <span className="hit-doc">{f.documentName}</span>
                    </div>
                    <CitedText answer={f.text} targetPrefix="matrix-item" />
                  </li>
                ))}
              </ol>
            </>
          )}

          {table.synthesis && (
            <>
              <h3 className="evidence-title">Cross-document synthesis</h3>
              <div className="evidence-card synthesis-card">
                <CitedText answer={table.synthesis} targetPrefix="matrix-item" />
              </div>
            </>
          )}
        </div>
      )}

      {history.length > 0 && (
        <div className="card">
          <h2>Saved evidence tables</h2>
          <ul className="history-list">
            {history.map((h) => (
              <li key={h.id} className="history-row">
                <button type="button" className="history-item" onClick={() => void openSaved(h.id)}>
                  {h.hasAi && <span className="chip subtle">AI</span>}
                  <span className="history-question">{h.question}</span>
                  <span className="tiny muted">{new Date(h.createdAt).toLocaleString()}</span>
                </button>
                <button
                  type="button"
                  className="btn ghost tiny-btn"
                  onClick={() => void deleteSaved(h.id)}
                  aria-label={`Delete table ${h.question}`}
                >
                  Delete
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
