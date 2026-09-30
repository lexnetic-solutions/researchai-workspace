export function TitleBar() {
  return (
    <header className="titlebar">
      <div className="titlebar-brand">
        <span className="brand-mark" aria-hidden="true">
          RA
        </span>
        <span className="brand-name">ResearchAI</span>
        <span className="brand-tag">Workspace</span>
      </div>
      <div className="titlebar-search" title="Global library search arrives in Phase 1">
        <span aria-hidden="true">⌕</span>
        <input
          type="search"
          placeholder="Search library (coming in Phase 1)"
          disabled
          aria-label="Search library"
        />
      </div>
    </header>
  );
}
