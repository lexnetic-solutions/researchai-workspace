import { useState } from 'react';
import { backend } from '../backend/client';
import { Modal } from '../components/Modal';
import { EmptyState } from '../components/EmptyState';
import { useStore } from '../state/store';

export function ProjectsView() {
  const { projects, activeProject, setActiveProjectId, reloadProjects, pushToast } = useStore();
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [busy, setBusy] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

  const pendingDeleteProject = projects.find((p) => p.id === pendingDelete) ?? null;

  async function submitCreate() {
    if (!name.trim()) {
      pushToast('error', 'Project name is required.');
      return;
    }
    setBusy(true);
    try {
      const created = await backend.createProject(name.trim(), description.trim() || undefined);
      await reloadProjects();
      setActiveProjectId(created.id);
      pushToast('success', `Project “${created.name}” created.`);
      setCreating(false);
      setName('');
      setDescription('');
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function confirmDelete() {
    if (!pendingDelete) return;
    setBusy(true);
    try {
      await backend.deleteProject(pendingDelete);
      if (activeProject?.id === pendingDelete) setActiveProjectId(null);
      await reloadProjects();
      pushToast('success', 'Project deleted.');
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
      setPendingDelete(null);
    }
  }

  return (
    <section className="view">
      <header className="view-head row">
        <div>
          <h1>Projects</h1>
          <p className="view-sub">Group documents, notes and analyses per research effort.</p>
        </div>
        <button type="button" className="btn primary" onClick={() => setCreating(true)}>
          + New project
        </button>
      </header>

      {projects.length === 0 ? (
        <EmptyState
          title="No projects"
          hint="Create a project to start importing research documents."
          action={
            <button type="button" className="btn primary" onClick={() => setCreating(true)}>
              + New project
            </button>
          }
        />
      ) : (
        <ul className="project-list">
          {projects.map((p) => (
            <li
              key={p.id}
              className={`project-card${activeProject?.id === p.id ? ' active' : ''}`}
            >
              <div className="project-card-main">
                <button type="button" className="project-name link" onClick={() => setActiveProjectId(p.id)}>
                  {p.name}
                </button>
                {p.description && <p className="muted">{p.description}</p>}
                <p className="tiny muted">
                  Created {new Date(p.createdAt).toLocaleDateString()} · Updated{' '}
                  {new Date(p.updatedAt).toLocaleDateString()}
                  {activeProject?.id === p.id ? ' · Active' : ''}
                </p>
              </div>
              <div className="project-card-actions">
                <button type="button" className="btn ghost" onClick={() => setActiveProjectId(p.id)}>
                  {activeProject?.id === p.id ? 'Active' : 'Set active'}
                </button>
                <button type="button" className="btn danger-ghost" onClick={() => setPendingDelete(p.id)}>
                  Delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {creating && (
        <Modal title="New project" onClose={() => setCreating(false)}>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void submitCreate();
            }}
          >
            <label className="field">
              <span>Name</span>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Thesis — Climate Adaptation"
                autoFocus
              />
            </label>
            <label className="field">
              <span>Description (optional)</span>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                rows={3}
                placeholder="What is this research effort about?"
              />
            </label>
            <div className="modal-actions">
              <button type="button" className="btn ghost" onClick={() => setCreating(false)}>
                Cancel
              </button>
              <button type="submit" className="btn primary" disabled={busy}>
                {busy ? 'Creating…' : 'Create project'}
              </button>
            </div>
          </form>
        </Modal>
      )}

      {pendingDeleteProject && (
        <Modal title="Delete project?" onClose={() => setPendingDelete(null)}>
          <p>
            This permanently removes the <strong>{pendingDeleteProject.name}</strong> project,
            its document records and the managed copies of its imported files from the
            workspace. Files you imported with “link original” stay where they are on
            disk.
          </p>
          <div className="modal-actions">
            <button type="button" className="btn ghost" onClick={() => setPendingDelete(null)}>
              Cancel
            </button>
            <button type="button" className="btn danger" disabled={busy} onClick={() => void confirmDelete()}>
              {busy ? 'Deleting…' : 'Delete project'}
            </button>
          </div>
        </Modal>
      )}
    </section>
  );
}
