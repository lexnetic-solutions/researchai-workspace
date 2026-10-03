import { useEffect, useState } from 'react';

/**
 * Scratchpad notes. Kept in app storage rather than the database so the
 * workflow works with or without a project selected.
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
          Free-form research notes, saved on this machine only.
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
