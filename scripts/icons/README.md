# App icons

Phase 0 ships a placeholder vector icon: `apps/desktop/public/app-icon.svg`
(also staged to `apps/desktop/src-tauri/icons/icon.svg` by `pnpm setup`).

Before release (Phase 9):

1. Replace the placeholder with final artwork as a 1024×1024 PNG at
   `scripts/icons/source/icon-1024.png`.
2. Run `pnpm --filter @researchai/desktop tauri icon scripts/icons/source/icon-1024.png`
   — this generates all platform icon sets (icns/ico/png).
3. Point `bundle.icon` in `apps/desktop/src-tauri/tauri.conf.json` at the
   generated set (the CLI prints the exact list).
