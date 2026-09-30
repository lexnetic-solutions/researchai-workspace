import { useStore, type ViewId } from '../state/store';

interface NavItem {
  readonly id: ViewId;
  readonly label: string;
  readonly glyph: string;
  readonly phase?: number;
}

/** Phase 0 navigation (spec §25). Later phases fill in disabled items. */
const NAV: readonly NavItem[] = [
  { id: 'home', label: 'Home', glyph: '⌂' },
  { id: 'projects', label: 'Projects', glyph: '❏' },
  { id: 'documents', label: 'Library', glyph: '▤' },
  { id: 'search', label: 'Search', glyph: '⌕' },
  { id: 'research', label: 'Research AI', glyph: '✦' },
  { id: 'notes', label: 'Notes', glyph: '✎' },
  { id: 'bibliography', label: 'Bibliography', glyph: '❡' },
  { id: 'evidence', label: 'Evidence', glyph: '⧉' },
  { id: 'audio', label: 'Audio', glyph: '♪', phase: 7 },
  { id: 'exports', label: 'Exports', glyph: '⤓', phase: 6 },
  { id: 'settings', label: 'Settings', glyph: '⚙' },
];

export function Sidebar() {
  const { view, setView, activeProject } = useStore();

  return (
    <nav className="sidebar" aria-label="Primary">
      <ul className="nav-list">
        {NAV.map((item) => {
          const disabled = item.phase !== undefined;
          return (
            <li key={item.id}>
              <button
                type="button"
                className={`nav-item${view === item.id ? ' active' : ''}`}
                disabled={disabled}
                onClick={() => setView(item.id)}
                title={disabled ? `Arrives in Phase ${item.phase}` : item.label}
              >
                <span className="nav-glyph" aria-hidden="true">
                  {item.glyph}
                </span>
                <span className="nav-label">{item.label}</span>
                {disabled && <span className="nav-badge">P{item.phase}</span>}
              </button>
            </li>
          );
        })}
      </ul>
      {activeProject && (
        <div className="sidebar-project" title={activeProject.description ?? activeProject.name}>
          <div className="sidebar-project-label">Active project</div>
          <div className="sidebar-project-name">{activeProject.name}</div>
        </div>
      )}
    </nav>
  );
}
