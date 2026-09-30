import { useStore } from '../state/store';
import { EmptyState } from '../components/EmptyState';

const FORMATS = ['PDF', 'DOCX', 'MD', 'TXT', 'CSV', 'XLSX', 'PPTX', 'MP3', 'WAV'] as const;

const TEMPLATES = [
  'Quick Summary',
  'Deep Analysis',
  'Critical Analysis',
  'Evidence Matrix',
  'Literature Review Draft',
  'Research Notes',
  'Comparison Report',
  'Bibliography',
  'Presentation',
  'Audio Summary',
  'Research Podcast',
] as const;

export function ExportsView() {
  const { setView } = useStore();

  return (
    <section className="view">
      <header className="view-head">
        <h1>Exports</h1>
        <p className="view-sub">Turn analyses into academic artefacts, fully offline.</p>
      </header>

      <div className="card notice">
        <h2>Export centre arrives in Phase 6</h2>
        <p>
          Rendering pipelines: DOCX (python-docx), XLSX (openpyxl), PPTX
          (python-pptx), PDF and Markdown from the same content model.
        </p>
        <button type="button" className="btn ghost" onClick={() => setView('library')}>
          Meanwhile: import documents
        </button>
      </div>

      <div className="two-col">
        <div className="card">
          <h2>Formats</h2>
          <div className="chip-row">
            {FORMATS.map((f) => (
              <span key={f} className="chip">
                {f}
              </span>
            ))}
          </div>
        </div>
        <div className="card">
          <h2>Templates</h2>
          <ul className="check-list two">
            {TEMPLATES.map((t) => (
              <li key={t}>{t}</li>
            ))}
          </ul>
        </div>
      </div>

      <EmptyState title="No exports yet" hint="Analyses can be exported once documents are processed." />
    </section>
  );
}
