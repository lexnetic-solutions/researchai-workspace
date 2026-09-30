import { useEffect, useRef, useState } from 'react';
import { backend } from '../backend/client';
import { useStore } from '../state/store';

interface Command {
  readonly id: string;
  readonly label: string;
  readonly run: () => void;
}

export function CommandPalette() {
  const { setView, pushToast, native } = useStore();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setOpen((prev) => !prev);
      }
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  const commands: readonly Command[] = [
    { id: 'go-home', label: 'Go to Home', run: () => setView('home') },
    { id: 'go-projects', label: 'Go to Projects', run: () => setView('projects') },
    { id: 'go-library', label: 'Go to Library', run: () => setView('library') },
    { id: 'go-notes', label: 'Go to Notes', run: () => setView('notes') },
    { id: 'go-settings', label: 'Go to Settings', run: () => setView('settings') },
    {
      id: 'import-files',
      label: 'Import documents…',
      run: () => {
        setView('library');
        pushToast('info', 'Document import lands in Phase 1.');
      },
    },
    {
      id: 'engine-health',
      label: 'Check document engine health',
      run: async () => {
        try {
          const ok = await backend.probeDocumentEngine();
          pushToast(ok ? 'success' : 'error', ok ? 'Document engine online.' : 'Document engine offline.');
        } catch {
          pushToast('error', 'Engine probe failed.');
        }
      },
    },
    {
      id: 'about-shell',
      label: native ? 'Native shell active' : 'Browser preview mode',
      run: () => pushToast('info', native ? 'Tauri IPC is live.' : 'Mock backend — run pnpm desktop:dev for the real one.'),
    },
  ];

  const filtered = commands.filter((c) =>
    c.label.toLowerCase().includes(query.trim().toLowerCase()),
  );

  if (!open) return null;

  return (
    <div className="palette-overlay" onClick={() => setOpen(false)}>
      <div className="palette" role="dialog" aria-label="Command palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="Type a command…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <ul className="palette-list">
          {filtered.map((c) => (
            <li key={c.id}>
              <button
                type="button"
                className="palette-item"
                onClick={() => {
                  setOpen(false);
                  setQuery('');
                  void c.run();
                }}
              >
                {c.label}
              </button>
            </li>
          ))}
          {filtered.length === 0 && <li className="palette-empty">No matching commands</li>}
        </ul>
      </div>
    </div>
  );
}
