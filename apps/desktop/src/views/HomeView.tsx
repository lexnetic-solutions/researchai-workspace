import { useStore } from '../state/store';
import { EmptyState } from '../components/EmptyState';

export function HomeView() {
  const { projects, activeProject, setActiveProjectId, setView, native } = useStore();

  return (
    <section className="view">
      <header className="view-head">
        <h1>Home</h1>
        <p className="view-sub">
          A private, offline-first research workspace. Your documents stay on this
          machine — {native ? 'managed locally by the desktop app' : 'preview mode runs entirely in-browser'}.
        </p>
      </header>

      <div className="home-grid">
        <div className="card">
          <h2>Active project</h2>
          {activeProject ? (
            <>
              <p className="card-title">{activeProject.name}</p>
              {activeProject.description && <p className="muted">{activeProject.description}</p>}
              <button type="button" className="btn" onClick={() => setView('library')}>
                Open Library
              </button>
            </>
          ) : (
            <>
              <p className="muted">No project selected.</p>
              {projects.length > 0 ? (
                <ul className="project-picker">
                  {projects.map((p) => (
                    <li key={p.id}>
                      <button
                        type="button"
                        className="link"
                        onClick={() => {
                          setActiveProjectId(p.id);
                          setView('library');
                        }}
                      >
                        {p.name}
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <button type="button" className="btn" onClick={() => setView('projects')}>
                  Create your first project
                </button>
              )}
            </>
          )}
        </div>

        <div className="card">
          <h2>Start here</h2>
          <ol className="steps">
            <li>Create a project for your thesis, coursework or review.</li>
            <li>Import a folder of PDFs, DOCX, slides or notes (Phase 1).</li>
            <li>Read, search and annotate your library.</li>
            <li>Ask questions with traceable citations (Phase 3).</li>
            <li>Export summaries and evidence matrices (Phase 6).</li>
          </ol>
        </div>

        <div className="card">
          <h2>Workspace status</h2>
          <ul className="status-lines">
            <li>
              <span className="status-dot ok" /> Local storage ready
            </li>
            <li>
              <span className="status-dot unknown" /> Document engine appears online after first
              launch of the sidecar
            </li>
            <li>
              <span className="status-dot ok" /> No cloud services configured
            </li>
          </ul>
          <button type="button" className="btn ghost" onClick={() => setView('settings')}>
            Open Settings &amp; Diagnostics
          </button>
        </div>
      </div>

      <div className="card wide">
        <h2>Planned in later phases</h2>
        <div className="phase-grid">
          {[
            ['Phase 1', 'Document import & parsing'],
            ['Phase 2', 'Hybrid semantic search'],
            ['Phase 3', 'Local AI (llama.cpp)'],
            ['Phase 4', 'Cross-document comparison'],
            ['Phase 5', 'Citations & bibliography'],
            ['Phase 6', 'Academic exports'],
            ['Phase 7', 'Lecture transcription'],
            ['Phase 8', 'Audio summaries & podcast'],
            ['Phase 9', 'Platform installers'],
          ].map(([phase, label]) => (
            <div key={phase} className="phase-cell">
              <span className="phase-tag">{phase}</span>
              <span>{label}</span>
            </div>
          ))}
        </div>
      </div>

      {projects.length === 0 && (
        <EmptyState
          title="No projects yet"
          hint="Projects group your documents, notes, evidence and exports."
        />
      )}
    </section>
  );
}
