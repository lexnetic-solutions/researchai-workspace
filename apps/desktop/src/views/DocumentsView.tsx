import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  BibliographyUpdate,
  DocumentSummary,
  IngestionStage,
  RefType,
} from '@researchai/shared-types';
import { backend } from '../backend/client';
import { EmptyState } from '../components/EmptyState';
import { Modal } from '../components/Modal';
import { useStore } from '../state/store';

const STATUS_LABEL: Record<IngestionStage, string> = {
  waiting: 'Waiting',
  parsing: 'Parsing',
  ocr: 'OCR',
  indexing: 'Indexing',
  analysing: 'Analysing',
  ready: 'Ready',
  failed: 'Failed',
};

function StatusBadge({ status }: { status: IngestionStage }) {
  return <span className={`chip status-${status}`}>{STATUS_LABEL[status] ?? status}</span>;
}

export function DocumentsView() {
  const { activeProject, pushToast, settings, native } = useStore();
  const [documents, setDocuments] = useState<DocumentSummary[]>([]);
  const [importing, setImporting] = useState(false);
  const [reader, setReader] = useState<{ doc: DocumentSummary; text: string } | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<DocumentSummary | null>(null);
  const [bibDraft, setBibDraft] = useState<BibliographyUpdate | null>(null);
  const [savingBib, setSavingBib] = useState(false);
  const pollRef = useRef<number | null>(null);

  const refresh = useCallback(async () => {
    if (!activeProject) return;
    try {
      setDocuments(await backend.listDocuments(activeProject.id));
    } catch {
      /* transient poll errors are non-fatal; the next tick retries */
    }
  }, [activeProject]);

  // Poll while any document is still moving through the pipeline.
  useEffect(() => {
    void refresh();
    const busy = documents.some((d) => ['waiting', 'parsing', 'ocr', 'indexing', 'analysing'].includes(d.indexingStatus));
    if (!busy) return;
    pollRef.current = window.setInterval(() => void refresh(), 1200);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
    };
  }, [refresh, documents]);

  async function runImport(kind: 'folder' | 'files') {
    if (!activeProject) return;
    setImporting(true);
    try {
      const picked =
        kind === 'folder' ? await backend.pickFolder() : await backend.pickDocuments();
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      if (paths.length === 0) return;

      const summary = await backend.importDocuments(
        activeProject.id,
        paths,
        settings?.defaultImportMode ?? 'managed-copy',
      );

      const parts: string[] = [];
      if (summary.imported > 0) parts.push(`${summary.imported} imported`);
      if (summary.duplicates > 0) parts.push(`${summary.duplicates} duplicate(s) skipped`);
      if (summary.errors.length > 0) parts.push(`${summary.errors.length} failed`);

      if (summary.errors.length > 0) {
        pushToast('error', `Import problems — ${summary.errors.join('; ')}`);
      } else if (parts.length > 0) {
        pushToast('success', `Import complete: ${parts.join(', ')}.`);
      } else {
        pushToast('info', 'Nothing to import.');
      }
      await refresh();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setImporting(false);
    }
  }

  async function openReader(doc: DocumentSummary) {
    try {
      const text = await backend.getDocumentText(doc.id);
      setReader({ doc, text });
      setBibDraft(null);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  function beginEditMetadata() {
    if (!reader) return;
    const { doc } = reader;
    setBibDraft({
      title: doc.title,
      authors: doc.authors,
      year: doc.year,
      doi: doc.doi,
      journal: doc.journal,
      volume: doc.volume,
      issue: doc.issue,
      pages: doc.pages,
      publisher: doc.publisher,
      url: doc.url,
      refType: doc.refType,
    });
  }

  async function saveMetadata() {
    if (!reader || !bibDraft) return;
    setSavingBib(true);
    try {
      await backend.updateBibliography(reader.doc.id, bibDraft);
      pushToast('success', 'Citation metadata updated.');
      const updated = {
        ...reader.doc,
      } as { -readonly [K in keyof DocumentSummary]: DocumentSummary[K] };
      if (bibDraft.title !== undefined) updated.title = bibDraft.title;
      if (bibDraft.authors !== undefined) updated.authors = bibDraft.authors;
      if (bibDraft.year !== undefined) updated.year = bibDraft.year;
      if (bibDraft.doi !== undefined) updated.doi = bibDraft.doi;
      if (bibDraft.journal !== undefined) updated.journal = bibDraft.journal;
      if (bibDraft.volume !== undefined) updated.volume = bibDraft.volume;
      if (bibDraft.issue !== undefined) updated.issue = bibDraft.issue;
      if (bibDraft.pages !== undefined) updated.pages = bibDraft.pages;
      if (bibDraft.publisher !== undefined) updated.publisher = bibDraft.publisher;
      if (bibDraft.url !== undefined) updated.url = bibDraft.url;
      if (bibDraft.refType) updated.refType = bibDraft.refType;
      setReader({ doc: updated, text: reader.text });
      setBibDraft(null);
      void refresh();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setSavingBib(false);
    }
  }

  async function retry(doc: DocumentSummary) {
    try {
      await backend.retryDocument(doc.id);
      pushToast('info', `Requeued “${doc.fileName}” for parsing.`);
      await refresh();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function remove(doc: DocumentSummary) {
    try {
      await backend.deleteDocument(doc.id);
      pushToast('success', `Removed “${doc.fileName}”.`);
      setConfirmDelete(null);
      await refresh();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  if (!activeProject) {
    return (
      <section className="view">
        <header className="view-head">
          <h1>Library</h1>
          <p className="view-sub">Select a project to manage its document library.</p>
        </header>
        <EmptyState title="No active project" hint="Create or select a project first." />
      </section>
    );
  }

  const readyCount = documents.filter((d) => d.indexingStatus === 'ready').length;

  return (
    <section className="view">
      <header className="view-head row">
        <div>
          <h1>Library</h1>
          <p className="view-sub">
            {activeProject.name} — {documents.length} document(s), {readyCount} ready.
            {!native && ' (Browser preview uses simulated parsing.)'}
          </p>
        </div>
        <div className="row-actions">
          <button type="button" className="btn" disabled={importing} onClick={() => void runImport('files')}>
            Import files…
          </button>
          <button type="button" className="btn primary" disabled={importing} onClick={() => void runImport('folder')}>
            Import folder…
          </button>
        </div>
      </header>

      {documents.length === 0 ? (
        <EmptyState
          title="No documents yet"
          hint="Import PDF, DOCX, PPTX, XLSX, MD, TXT or HTML files. Duplicates are detected by checksum and skipped automatically. Start the document engine (pnpm engine:run) so parsing can run."
        />
      ) : (
        <ul className="doc-list">
          {documents.map((doc) => (
            <li key={doc.id} className="doc-row">
              <div className="doc-main">
                <div className="doc-title-line">
                  <button type="button" className="link doc-name" onClick={() => void openReader(doc)}>
                    {doc.title || doc.fileName}
                  </button>
                  <StatusBadge status={doc.indexingStatus as IngestionStage} />
                </div>
                <p className="tiny muted">
                  {doc.fileName} · {doc.documentType.toUpperCase()}
                  {doc.pageCount != null && ` · ${doc.pageCount} pages`}
                  {doc.chunkCount > 0 && ` · ${doc.chunkCount} chunks`}
                  {' · '}
                  {new Date(doc.importedAt).toLocaleString()}
                </p>
                {doc.statusDetail && <p className="tiny warn-text">{doc.statusDetail}</p>}
              </div>
              <div className="doc-actions">
                {doc.indexingStatus === 'failed' && (
                  <button type="button" className="btn ghost" onClick={() => void retry(doc)}>
                    Retry
                  </button>
                )}
                <button
                  type="button"
                  className="btn danger-ghost"
                  onClick={() => setConfirmDelete(doc)}
                  aria-label={`Delete ${doc.fileName}`}
                >
                  Delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {reader && (
        <Modal title={reader.doc.title || reader.doc.fileName} onClose={() => setReader(null)}>
          <div className="card meta-card">
            <div className="hit-head">
              <div className="tiny muted">
                {(reader.doc.authors || 'No authors recorded')
                + (reader.doc.year != null ? ` · ${reader.doc.year}` : '')
                + (reader.doc.journal ? ` · ${reader.doc.journal}` : '')}
              </div>
              {!bibDraft && (
                <button type="button" className="btn ghost tiny-btn" onClick={beginEditMetadata}>
                  Edit citation metadata
                </button>
              )}
            </div>
            {(reader.doc.doi || reader.doc.url) && (
              <p className="tiny mono">
                {reader.doc.doi ? `doi: ${reader.doc.doi}` : reader.doc.url}
              </p>
            )}

            {bibDraft && (
              <div className="meta-form">
                <label>
                  <span className="diag-label">Authors (semicolon-separated)</span>
                  <input
                    className="search-input"
                    value={bibDraft.authors ?? ''}
                    placeholder="Jane Doe; John Smith"
                    onChange={(e) => setBibDraft({ ...bibDraft, authors: e.target.value })}
                  />
                </label>
                <div className="ai-settings-grid">
                  <label>
                    <span className="diag-label">Title</span>
                    <input
                      className="search-input"
                      value={bibDraft.title ?? ''}
                      onChange={(e) => setBibDraft({ ...bibDraft, title: e.target.value })}
                    />
                  </label>
                  <label>
                    <span className="diag-label">Year</span>
                    <input
                      type="number"
                      className="search-input"
                      value={bibDraft.year ?? ''}
                      onChange={(e) =>
                        setBibDraft({
                          ...bibDraft,
                          year: e.target.value ? Number(e.target.value) : null,
                        })
                      }
                    />
                  </label>
                  <label>
                    <span className="diag-label">Reference type</span>
                    <select
                      className="search-input"
                      value={bibDraft.refType ?? 'article'}
                      onChange={(e) =>
                        setBibDraft({ ...bibDraft, refType: e.target.value as RefType })
                      }
                    >
                      {(['article', 'book', 'chapter', 'report', 'webpage', 'thesis'] as const).map(
                        (t) => (
                          <option key={t} value={t}>
                            {t}
                          </option>
                        ),
                      )}
                    </select>
                  </label>
                </div>
                <div className="ai-settings-grid">
                  <label>
                    <span className="diag-label">Journal / container</span>
                    <input
                      className="search-input"
                      value={bibDraft.journal ?? ''}
                      onChange={(e) => setBibDraft({ ...bibDraft, journal: e.target.value })}
                    />
                  </label>
                  <label>
                    <span className="diag-label">Volume</span>
                    <input
                      className="search-input"
                      value={bibDraft.volume ?? ''}
                      onChange={(e) => setBibDraft({ ...bibDraft, volume: e.target.value })}
                    />
                  </label>
                  <label>
                    <span className="diag-label">Issue</span>
                    <input
                      className="search-input"
                      value={bibDraft.issue ?? ''}
                      onChange={(e) => setBibDraft({ ...bibDraft, issue: e.target.value })}
                    />
                  </label>
                  <label>
                    <span className="diag-label">Pages</span>
                    <input
                      className="search-input"
                      value={bibDraft.pages ?? ''}
                      onChange={(e) => setBibDraft({ ...bibDraft, pages: e.target.value })}
                    />
                  </label>
                  <label>
                    <span className="diag-label">Publisher</span>
                    <input
                      className="search-input"
                      value={bibDraft.publisher ?? ''}
                      onChange={(e) => setBibDraft({ ...bibDraft, publisher: e.target.value })}
                    />
                  </label>
                  <label>
                    <span className="diag-label">DOI or URL</span>
                    <input
                      className="search-input"
                      value={bibDraft.doi ?? bibDraft.url ?? ''}
                      onChange={(e) => setBibDraft({ ...bibDraft, doi: e.target.value })}
                    />
                  </label>
                </div>
                <div className="field-row" style={{ marginTop: '0.5rem' }}>
                  <button
                    type="button"
                    className="btn primary"
                    disabled={savingBib}
                    onClick={() => void saveMetadata()}
                  >
                    {savingBib ? 'Saving…' : 'Save metadata'}
                  </button>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => setBibDraft(null)}
                  >
                    Cancel
                  </button>
                </div>
                <p className="tiny muted">
                  Your corrections are authoritative: extraction hints never overwrite them
                  (spec §31).
                </p>
              </div>
            )}
          </div>
          <p className="tiny muted">
            Extracted text ({reader.text.length.toLocaleString()} characters). The page-accurate
            PDF viewer arrives in a later phase — page markers are preserved for citations.
          </p>
          <pre className="reader-text">{reader.text}</pre>
        </Modal>
      )}

      {confirmDelete && (
        <Modal title="Remove document?" onClose={() => setConfirmDelete(null)}>
          <p>
            This deletes <strong>{confirmDelete.fileName}</strong> from the project library:
            the database record, its parsed chunks, and the managed copy in the workspace
            (original files on your disk are never touched).
          </p>
          <div className="modal-actions">
            <button type="button" className="btn ghost" onClick={() => setConfirmDelete(null)}>
              Cancel
            </button>
            <button type="button" className="btn danger" onClick={() => void remove(confirmDelete)}>
              Remove document
            </button>
          </div>
        </Modal>
      )}
    </section>
  );
}
