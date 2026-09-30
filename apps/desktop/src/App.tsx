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
  const { loading, backendError, view } = useStore();

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
