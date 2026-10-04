import { useEffect } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { convertFileSrc } from '@tauri-apps/api/core';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { backend } from './backend/client';
import { Sidebar } from './components/Sidebar';
import { StatusBar } from './components/StatusBar';
import { TitleBar } from './components/TitleBar';
import { ToastStack } from './components/ToastStack';
import { CommandPalette } from './components/CommandPalette';
import { HomeView } from './views/HomeView';
import { ProjectsView } from './views/ProjectsView';
import { DocumentsView } from './views/DocumentsView';
import { SearchView } from './views/SearchView';
import { ResearchView } from './views/ResearchView';
import { NotesView } from './views/NotesView';
import { BibliographyView } from './views/BibliographyView';
import { EvidenceView } from './views/EvidenceView';
import { AudioView } from './views/AudioView';
import { ExportsView } from './views/ExportsView';
import { SettingsView } from './views/SettingsView';
import { useStore } from './state/store';

function CurrentView() {
  const { view } = useStore();
  switch (view) {
    case 'home':
      return <HomeView />;
    case 'projects':
      return <ProjectsView />;
    case 'documents':
    case 'library':
      return <DocumentsView />;
    case 'search':
      return <SearchView />;
    case 'research':
      return <ResearchView />;
    case 'notes':
      return <NotesView />;
    case 'evidence':
      return <EvidenceView />;
    case 'bibliography':
      return <BibliographyView />;
    case 'audio':
      return <AudioView />;
    case 'exports':
      return <ExportsView />;
    case 'settings':
      return <SettingsView />;
  }
}

export default function App() {
  const { loading, backendError, view, native, requestImport, setView, pushToast, activeProject } =
    useStore();

  // Drag & drop import: Tauri delivers native drops as events (the webview's
  // own HTML5 drop is disabled), so without this listener dropped documents
  // silently do nothing.
  useEffect(() => {
    if (!native) return;
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== 'drop' || event.payload.paths.length === 0) return;
        if (!activeProject) {
          pushToast('error', 'Select or create a project before dropping documents in.');
          setView('projects');
          return;
        }
        setView('library');
        requestImport('files', event.payload.paths);
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* drag events unavailable (browser preview) — nothing to clean up */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [native, activeProject, requestImport, setView, pushToast]);

  // Audio playback self-check: a media failure in the packaged build is
  // invisible (the player just shows "Error"), so verify once at startup
  // that a narration WAV loads through the asset protocol the player uses —
  // reporting the page origin, the enforced CSP header, whether a duplicate
  // meta CSP exists, and the MediaError code (if any) to the app log.
  // Silent when no narration has been rendered yet.
  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    void (async () => {
      try {
        await backend.logFrontend(`audio self-check: page=${window.location.href}`);
        try {
          const head = await fetch(window.location.href, { method: 'GET' });
          const csp = head.headers.get('content-security-policy');
          await backend.logFrontend(
            `audio self-check: csp-header=${csp ? csp.slice(0, 600) : 'NONE'}`,
          );
          const meta = document.querySelector('meta[http-equiv="Content-Security-Policy"]');
          await backend.logFrontend(
            `audio self-check: meta-csp=${meta ? (meta.getAttribute('content') ?? '') : 'none'}`,
          );
        } catch (err) {
          await backend.logFrontend(`audio self-check: fetch(location) failed: ${String(err)}`);
        }
        const files = await backend.listExports();
        const wav = files.find((f) => f.name.startsWith('tts-') && f.name.endsWith('.wav'));
        if (!wav || cancelled) return;
        const src = convertFileSrc(wav.path);
        await new Promise<void>((resolve) => {
          const el = new Audio();
          el.preload = 'metadata';
          const done = () => resolve();
          el.onloadedmetadata = () => {
            void backend
              .logFrontend(
                `audio self-check: OK duration=${
                  Number.isFinite(el.duration) ? el.duration.toFixed(1) : el.duration
                }s src=${src}`,
              )
              .then(done);
          };
          el.onerror = () => {
            void backend
              .logFrontend(
                `audio self-check: FAILED code=${el.error?.code ?? '?'} message=${
                  el.error?.message ?? '?'
                } src=${src}`,
              )
              .then(done);
          };
          el.src = src;
          window.setTimeout(done, 8000);
        });
      } catch (err) {
        void backend.logFrontend(`audio self-check: error ${String(err)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [native]);

  return (
    <div className="app-shell">
      <TitleBar />
      <div className="app-body">
        <Sidebar />
        <main className="workspace" aria-label="Workspace">
          {loading ? (
            <div className="workspace-loading">Loading workspace…</div>
          ) : backendError ? (
            <div className="workspace-error" role="alert">
              <h2>Backend unavailable</h2>
              <p>{backendError}</p>
              <p className="hint">
                If you launched the browser preview without the Python document
                engine or the native shell, this may be expected. Try the desktop
                app via <code>pnpm desktop:dev</code>.
              </p>
            </div>
          ) : (
            <div className="view-scroll" key={view}>
              <CurrentView />
            </div>
          )}
        </main>
      </div>
      <StatusBar />
      <ToastStack />
      <CommandPalette />
    </div>
  );
}
