import { useState } from 'react';
import { useStore } from '../state/store';

export function TitleBar() {
  const { setView, setSearchSeed } = useStore();
  const [query, setQuery] = useState('');

  function submit() {
    const q = query.trim();
    if (!q) return;
    setSearchSeed(q);
    setView('search');
  }

  return (
    <header className="titlebar">
      <div className="titlebar-brand">
        <span className="brand-mark" aria-hidden="true">
          RA
        </span>
        <span className="brand-name">ResearchAI</span>
        <span className="brand-tag">Workspace</span>
      </div>
      <div className="titlebar-search" title="Type a query and press Enter to search the library">
        <span aria-hidden="true">⌕</span>
        <input
          type="search"
          placeholder="Search library…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') submit();
          }}
          aria-label="Search library"
        />
      </div>
    </header>
  );
}
