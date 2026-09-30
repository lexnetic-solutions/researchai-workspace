import { useEffect, useState } from 'react';

/**
 * Phase 0 scratchpad. The notes DB table (spec §12) lands with the document
 * model in Phase 1; until then this is a per-machine draft area so the
 * workflow is testable end-to-end without the parser.
 */
const DRAFT_KEY = 'researchai.notes.draft.v1';

export function NotesView() {
  const [draft, setDraft] = useState('');

  useEffect(() => {
    setDraft(window.localStorage.getItem(DRAFT_KEY) ?? '');
  }, []);

  function update(value: string) {
    setDraft(value);
    window.localStorage.setItem(DRAFT_KEY, value);
  }

  return (
    <section className="view">
      <header className="view-head">
        <h1>Notes</h1>
        <p className="view-sub">
          Free-form research notes. Structured, per-document notes arrive in Phase 1.
        </p>
      </header>
      <div className="card grow">
        <label className="field">
          <span>Scratchpad (saved locally on this machine)</span>
          <textarea
            rows={16}
            value={draft}
            onChange={(e) => update(e.target.value)}
            placeholder={'Key claims to verify…\nContradictions spotted between paper A and B…'}
          />
        </label>
        <p className="tiny muted">
          Draft autosaves to browser/app storage. It is not uploaded anywhere.
        </p>
      </div>
    </section>
  );
}
