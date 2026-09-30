import { useCallback, useEffect, useState } from 'react';
import type { CitationStyle, FormattedReference } from '@researchai/shared-types';
import { backend } from '../backend/client';
import { EmptyState } from '../components/EmptyState';
import { useStore } from '../state/store';

const STYLES: readonly { id: CitationStyle; label: string }[] = [
  { id: 'apa', label: 'APA 7' },
  { id: 'harvard', label: 'Harvard' },
  { id: 'chicago', label: 'Chicago' },
];

export function BibliographyView() {
  const { activeProject, pushToast } = useStore();
  const [style, setStyle] = useState<CitationStyle>('apa');
  const [refs, setRefs] = useState<FormattedReference[]>([]);
  const [loading, setLoading] = useState(false);

  const load = useCallback(
    async (s: CitationStyle) => {
      if (!activeProject) return;
      setLoading(true);
      try {
        setRefs(await backend.bibliographyList(activeProject.id, s));
      } catch (err) {
        pushToast('error', err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    },
    [activeProject, pushToast],
  );

  useEffect(() => {
    void load(style);
  }, [load, style]);

  if (!activeProject) {
    return (
      <section className="view">
        <header className="view-head">
          <h1>Bibliography</h1>
          <p className="view-sub">Select a project to format its references.</p>
        </header>
        <EmptyState title="No active project" hint="Create or select a project first." />
      </section>
    );
  }

  const incomplete = refs.filter((r) => r.incomplete).length;

  return (
    <section className="view">
      <header className="view-head row">
        <div>
          <h1>Bibliography</h1>
          <p className="view-sub">
            Formatted from your documents' citation metadata — deterministic, never invented
            by AI. Correct metadata in the Library reader; corrections are authoritative.
          </p>
        </div>
        <div className="field-row">
          {STYLES.map((s) => (
            <button
              key={s.id}
              type="button"
              className={`btn ${style === s.id ? 'primary' : 'ghost'}`}
              onClick={() => setStyle(s.id)}
            >
              {s.label}
            </button>
          ))}
        </div>
      </header>

      {refs.length === 0 ? (
        <EmptyState
          title="No references yet"
          hint="Import documents to build the project bibliography."
        />
      ) : (
        <div className="card">
          <div className="search-meta">
            <span className="tiny muted">
              {refs.length} reference(s){incomplete > 0 ? ` · ${incomplete} incomplete` : ''} ·{' '}
              {loading ? 'refreshing…' : 'alphabetical'}
            </span>
            <button
              type="button"
              className="btn ghost tiny-btn"
              onClick={async () => {
                const text = refs.map((r) => r.reference).join('\n\n');
                try {
                  await navigator.clipboard.writeText(text);
                  pushToast('success', 'Bibliography copied to the clipboard.');
                } catch {
                  pushToast('error', 'Clipboard unavailable — select the text manually.');
                }
              }}
            >
              Copy all
            </button>
          </div>
          <ol className="bib-list">
            {refs.map((r) => (
              <li key={r.documentId} className="bib-entry">
                <span className="bib-reference">{r.reference}</span>
                {r.incomplete && (
                  <span
                    className="chip warn"
                    title="Title or year missing — fix it in the Library reader."
                  >
                    incomplete
                  </span>
                )}
                <span className="tiny muted bib-in-text">{r.inText}</span>
                <button
                  type="button"
                  className="btn ghost tiny-btn"
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(r.reference);
                      pushToast('success', 'Reference copied.');
                    } catch {
                      pushToast('error', 'Clipboard unavailable.');
                    }
                  }}
                >
                  Copy
                </button>
              </li>
            ))}
          </ol>
        </div>
      )}
    </section>
  );
}
