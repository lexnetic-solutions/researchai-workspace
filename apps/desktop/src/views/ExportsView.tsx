import { useCallback, useEffect, useState } from 'react';
import type {
  AnalysisSummary,
  CitationStyle,
  EvidenceSummary,
  ExportFile,
  ExportFormat,
  ExportKind,
  ExportProgressEvent,
  ExportStats,
} from '@researchai/shared-types';
import { backend, onExportProgress } from '../backend/client';
import { EmptyState } from '../components/EmptyState';
import { useStore } from '../state/store';

const FORMAT_LABEL: Partial<Record<ExportFormat, string>> = {
  markdown: 'Markdown (.md)',
  docx: 'Word (.docx)',
  pdf: 'PDF (.pdf)',
  bibtex: 'BibTeX (.bib)',
  ris: 'RIS (.ris)',
};

function formatBytes(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)} MB`;
  if (n >= 1_000) return `${Math.round(n / 1_000)} KB`;
  return `${n} B`;
}

export function ExportsView() {
  const { activeProject, pushToast, native } = useStore();
  const [kind, setKind] = useState<ExportKind>('bibliography');
  const [format, setFormat] = useState<ExportFormat>('markdown');
  const [style, setStyle] = useState<CitationStyle>('apa');
  const [analyses, setAnalyses] = useState<AnalysisSummary[]>([]);
  const [tables, setTables] = useState<EvidenceSummary[]>([]);
  const [sourceId, setSourceId] = useState<string>('');
  const [exporting, setExporting] = useState(false);
  const [progress, setProgress] = useState<ExportProgressEvent | null>(null);
  const [files, setFiles] = useState<ExportFile[]>([]);
  const [stats, setStats] = useState<ExportStats | null>(null);

  const reloadSources = useCallback(async () => {
    if (!activeProject) return;
    try {
      const [a, t] = await Promise.all([
        backend.aiListAnalyses(activeProject.id, 30),
        backend.evidenceList(activeProject.id, 30),
      ]);
      setAnalyses(a);
      setTables(t);
    } catch {
      /* sources are optional */
    }
  }, [activeProject]);

  const reloadFiles = useCallback(async () => {
    try {
      setFiles(await backend.listExports());
    } catch {
      setFiles([]);
    }
    try {
      setStats(await backend.exportsStats());
    } catch {
      setStats(null);
    }
  }, []);

  useEffect(() => {
    void reloadSources();
    void reloadFiles();
  }, [reloadSources, reloadFiles]);

  // Live status for long DOCX/PDF renders; the final "writing" phase also
  // refreshes the file list so finished exports appear without a reload.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void onExportProgress((ev) => {
      setProgress(ev);
      if (ev.phase === 'writing') {
        void reloadFiles();
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [reloadFiles]);

  async function runExport() {
    if (!activeProject) return;
    setExporting(true);
    setProgress(null);
    try {
      const needsSource = kind !== 'bibliography';
      if (needsSource && !sourceId) {
        pushToast('error', 'Pick an analysis or evidence table to export first.');
        return;
      }
      const result =
        kind === 'bibliography' && (format === 'bibtex' || format === 'ris')
          ? await backend.exportBibliography(activeProject.id, format)
          : await backend.exportDocument(
              activeProject.id,
              kind,
              needsSource ? sourceId : null,
              format,
              style,
            );
      pushToast('success', `Exported ${result.bytes.toLocaleString()} bytes.`);
      await reloadFiles();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setExporting(false);
      setProgress(null);
    }
  }

  async function reveal(path: string) {
    try {
      await backend.revealPath(path);
      if (!native) {
        pushToast('info', 'Reveal opens the OS file manager in the desktop app.');
      }
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function removeExport(path: string, name: string) {
    try {
      const next = await backend.deleteExport(path);
      setStats(next);
      await reloadFiles();
      pushToast('success', `Deleted ${name}.`);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  if (!activeProject) {
    return (
      <section className="view">
        <header className="view-head">
          <h1>Exports</h1>
          <p className="view-sub">Select a project to export its artefacts.</p>
        </header>
        <EmptyState title="No active project" hint="Create or select a project first." />
      </section>
    );
  }

  const sources =
    kind === 'analysis' ? analyses : kind === 'evidence_table' ? tables : [];

  return (
    <section className="view">
      <header className="view-head">
        <h1>Exports</h1>
        <p className="view-sub">
          Academic artefacts written into the managed workspace
          (<code>exports/</code>) — analyses, evidence matrices and the
          bibliography, in document or reference-manager formats.
        </p>
      </header>

      <div className="card">
        <h2>New export</h2>
        <div className="field-row" style={{ marginTop: '0.5rem' }}>
          <select
            className="search-scope"
            value={kind}
            onChange={(e) => {
              setKind(e.target.value as ExportKind);
              setSourceId('');
              if (e.target.value === 'bibliography' && format === 'pdf') setFormat('markdown');
            }}
            aria-label="Export kind"
          >
            <option value="bibliography">Bibliography</option>
            <option value="analysis">AI analysis</option>
            <option value="evidence_table">Evidence matrix</option>
          </select>
          <select
            className="search-scope"
            value={format}
            onChange={(e) => setFormat(e.target.value as ExportFormat)}
            aria-label="Export format"
          >
            {(kind === 'analysis'
              ? (['markdown', 'docx', 'pdf'] as const)
              : (['markdown', 'docx', 'pdf', 'bibtex', 'ris'] as const)
            ).map((f) => (
              <option key={f} value={f}>
                {FORMAT_LABEL[f] ?? f}
              </option>
            ))}
          </select>
          {(kind === 'bibliography' && format !== 'bibtex' && format !== 'ris') && (
            <select
              className="search-scope"
              value={style}
              onChange={(e) => setStyle(e.target.value as CitationStyle)}
              aria-label="Citation style"
            >
              <option value="apa">APA 7</option>
              <option value="harvard">Harvard</option>
              <option value="chicago">Chicago</option>
            </select>
          )}
        </div>

        {kind !== 'bibliography' && (
          <div className="field-row" style={{ marginTop: '0.5rem' }}>
            <select
              className="search-input"
              value={sourceId}
              onChange={(e) => setSourceId(e.target.value)}
              aria-label="Source item"
            >
              <option value="">
                Choose {kind === 'analysis' ? 'an analysis' : 'an evidence table'}…
              </option>
              {sources.map((s) => (
                <option key={s.id} value={s.id}>
                  {(s.question ?? '(untitled)').slice(0, 70)}
                </option>
              ))}
            </select>
          </div>
        )}

        <div className="field-row" style={{ marginTop: '0.75rem' }}>
          <button
            type="button"
            className="btn primary"
            disabled={exporting}
            onClick={() => void runExport()}
          >
            {exporting ? 'Exporting…' : 'Export'}
          </button>
          {!native && <span className="tiny muted">(preview writes mock entries)</span>}
        </div>
        {exporting && progress && (
          <p className="tiny muted" role="status" style={{ marginTop: '0.5rem' }}>
            {progress.label} — {progress.phase}
            {progress.elapsedSecs > 0 ? ` · ${progress.elapsedSecs}s` : ''}
          </p>
        )}
        {format === 'docx' || format === 'pdf' ? (
          <p className="tiny muted" style={{ marginTop: '0.5rem' }}>
            DOCX/PDF rendering uses the document engine sidecar.
          </p>
        ) : null}
      </div>

      <div className="card">
        <h2>Exported files</h2>
        {stats && (
          <p className="tiny muted">
            {stats.files} file{stats.files === 1 ? '' : 's'} ·{' '}
            {formatBytes(stats.totalBytes)} in the managed exports folder. Deleting here removes
            the file from disk.
          </p>
        )}
        {files.length === 0 ? (
          <p className="tiny muted">Nothing exported yet.</p>
        ) : (
          <ul className="history-list">
            {files.map((f) => (
              <li key={f.path} className="history-row">
                <button
                  type="button"
                  className="history-item"
                  onClick={() => void reveal(f.path)}
                  title={f.path}
                >
                  <span className="history-question">{f.name}</span>
                  <span className="tiny muted">{formatBytes(f.sizeBytes)}</span>
                </button>
                <button
                  type="button"
                  className="btn ghost tiny-btn"
                  aria-label={`Delete ${f.name}`}
                  onClick={() => void removeExport(f.path, f.name)}
                >
                  Delete
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
