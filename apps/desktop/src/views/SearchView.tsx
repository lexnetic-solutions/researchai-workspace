import { useCallback, useEffect, useRef, useState } from 'react';
import type { DocumentSummary, SearchHit } from '@researchai/shared-types';
import { backend } from '../backend/client';
import { EmptyState } from '../components/EmptyState';
import { useStore } from '../state/store';

type Scope = 'library' | 'project' | 'document';

export function SearchView() {
  const { activeProject, pushToast, native } = useStore();
  const [query, setQuery] = useState('');
  const [scope, setScope] = useState<Scope>('project');
  const [scopeDocs, setScopeDocs] = useState<DocumentSummary[]>([]);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [trace, setTrace] = useState<SearchResponseTrace | null>(null);
  const [searching, setSearching] = useState(false);
  const [showDebug, setShowDebug] = useState(false);
  const [status, setStatus] = useState<{
    embeddingEngine: string;
    totalChunks: number;
    embeddedChunks: number;
  } | null>(null);
  const debounce = useRef<number | null>(null);

  const loadScopeDocs = useCallback(async () => {
    if (!activeProject) return;
    try {
      setScopeDocs(await backend.listDocuments(activeProject.id));
    } catch {
      /* non-fatal */
    }
  }, [activeProject]);

  useEffect(() => {
    void loadScopeDocs();
    void backend
      .retrievalStatus()
      .then(setStatus)
      .catch(() => setStatus(null));
  }, [loadScopeDocs]);

  async function runSearch(q: string) {
    if (!q.trim()) return;
    setSearching(true);
    try {
      let ids: string[] = [];
      if (scope === 'project' && activeProject) {
        // Project scope = all docs of the active project that are ready.
        ids = scopeDocs.filter((d) => d.indexingStatus === 'ready').map((d) => d.id);
      } else if (scope === 'document') {
        ids = scopeDocs.filter((d) => d.indexingStatus === 'ready').slice(0, 1).map((d) => d.id);
      }
      const res = await backend.searchLibrary(q.trim(), ids, 12);
      setHits([...res.hits]);
      setTrace(res.trace);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setSearching(false);
    }
  }

  // Debounced live search as the user types.
  function onInput(value: string) {
    setQuery(value);
    if (debounce.current) window.clearTimeout(debounce.current);
    if (value.trim().length >= 2) {
      debounce.current = window.setTimeout(() => void runSearch(value), 350);
    } else {
      setHits(null);
      setTrace(null);
    }
  }

  if (!activeProject) {
    return (
      <section className="view">
        <header className="view-head">
          <h1>Search</h1>
          <p className="view-sub">Select a project to search its documents.</p>
        </header>
        <EmptyState title="No active project" hint="Create or select a project first." />
      </section>
    );
  }

  const readyCount = scopeDocs.filter((d) => d.indexingStatus === 'ready').length;

  return (
    <section className="view">
      <header className="view-head">
        <h1>Search</h1>
        <p className="view-sub">
          Hybrid retrieval over your library — keyword (FTS5) fused with semantic vector
          search, reranked and deduplicated. Every hit shows its channels and score.
        </p>
      </header>

      <div className="card">
        <div className="search-row">
          <input
            className="search-input"
            placeholder={`Search ${readyCount} indexed document(s)…`}
            value={query}
            onChange={(e) => onInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void runSearch(query);
            }}
            aria-label="Search query"
          />
          <select
            className="search-scope"
            value={scope}
            onChange={(e) => {
              setScope(e.target.value as Scope);
              setHits(null);
            }}
            aria-label="Search scope"
          >
            <option value="project">Active project</option>
            <option value="library">Entire library</option>
            <option value="document">First document</option>
          </select>
          <button
            type="button"
            className="btn primary"
            disabled={searching || !query.trim()}
            onClick={() => void runSearch(query)}
          >
            {searching ? 'Searching…' : 'Search'}
          </button>
        </div>
        <div className="search-meta">
          <span className="tiny muted">
            {status
              ? `${status.embeddedChunks}/${status.totalChunks} chunks embedded · engine: ${status.embeddingEngine}`
              : 'Index status unavailable (engine offline?)'}
          </span>
          <button type="button" className="btn ghost tiny-btn" onClick={() => setShowDebug((v) => !v)}>
            {showDebug ? 'Hide' : 'Show'} retrieval debug
          </button>
        </div>

        {showDebug && trace && (
          <div className="debug-panel">
            <h3>Retrieval trace</h3>
            <div className="debug-grid">
              <div><span className="diag-label">Query</span>{trace.query}</div>
              <div><span className="diag-label">Keyword candidates</span>{trace.keywordCandidates}</div>
              <div><span className="diag-label">Vector candidates</span>{trace.vectorCandidates}</div>
              <div><span className="diag-label">Fused & returned</span>{trace.fused} → {trace.returned}</div>
              <div><span className="diag-label">Embedding engine</span>{trace.embeddingEngine}</div>
              <div><span className="diag-label">Coverage</span>{trace.embeddingCoverage}</div>
            </div>
            {trace.warnings.length > 0 && (
              <ul className="debug-warnings">
                {trace.warnings.map((w) => (
                  <li key={w} className="tiny warn-text">{w}</li>
                ))}
              </ul>
            )}
          </div>
        )}
        {showDebug && !trace && (
          <p className="tiny muted">Run a search to capture a trace.</p>
        )}
      </div>

      {hits === null ? (
        <EmptyState
          title="Type to search"
          hint="Results fuse keyword matches (BM25) with semantic vector neighbours, deduplicated and capped per document for source diversity."
        />
      ) : hits.length === 0 ? (
        <EmptyState title="No results" hint="Try different terms, or widen the scope." />
      ) : (
        <ul className="hit-list">
          {hits.map((h) => (
            <li key={h.chunkId} className="hit-card">
              <div className="hit-head">
                <span className="hit-doc">{h.documentName}</span>
                {h.pageNumber != null && <span className="chip">p. {h.pageNumber}</span>}
                {h.sectionHeading && (
                  <span className="chip subtle">{h.sectionHeading}</span>
                )}
                <span className="hit-score" title="Reciprocal-rank fusion score">
                  {h.score.toFixed(4)}
                </span>
              </div>
              <p className="hit-text">{h.text}</p>
              <div className="hit-foot">
                {h.matchedBy.map((m) => (
                  <span key={m} className={`chip channel-${m}`}>{m}</span>
                ))}
                {h.vecDistance != null && (
                  <span className="tiny muted" title="Cosine distance (lower = closer)">
                    vec {h.vecDistance.toFixed(3)}
                  </span>
                )}
                {h.ftsRank != null && (
                  <span className="tiny muted" title="BM25 rank (closer to 0 = better)">
                    bm25 {h.ftsRank.toFixed(2)}
                  </span>
                )}
                {!native && <span className="tiny muted">(preview data)</span>}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

type SearchResponseTrace = {
  query: string;
  keywordCandidates: number;
  vectorCandidates: number;
  fused: number;
  returned: number;
  embeddingEngine: string;
  embeddingCoverage: string;
  warnings: readonly string[];
};
