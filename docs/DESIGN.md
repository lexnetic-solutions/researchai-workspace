# Design Concept

Visual and interaction design for ResearchAI Workspace. Extracted from the
Voicebox project study (github.com/jamiepine/voicebox, MIT — cloned to a
scratch dir outside this repo per spec §3) and adapted to this app's
academic identity.

## Identity

ResearchAI is a **reading-room tool**, not a studio toy. Where Voicebox
uses a warm amber accent on near-white/near-black with playful loaders,
ResearchAI uses a **cool ink-blue accent on paper tones** — the metaphor
is margin annotations on paper. Keep the restraint: no decorative
gradients, no emoji iconography in the shell, no animation over 200 ms.

| Token role | Light | Dark | Notes |
|---|---|---|---|
| `--bg` | `#f7f7f5` paper | `#16181d` | page ground |
| `--bg-panel` | `#ffffff` | `#1e2128` | cards, sidebars |
| `--bg-inset` | `#f0efec` | `#262a33` | code, inputs |
| `--accent` | `#1d4ed8` ink blue | `#6f9bff` | links, focus, primary actions |
| `--danger` / `--ok` / `--warn` | `#b3372f` / `#2e7d43` / `#8a6d1a` | softened | status chips only |

Voicebox's system (HSL pairs + `--radius` scale + chart palette) is the
reference architecture; ours maps 1:1 onto `apps/desktop/src/styles/app.css`
custom properties. Both apps share the pattern: **every semantic colour is
a variable with a light value in `:root` and a dark override** — never
hard-code a colour in a component.

## Extraction rules (what came from Voicebox, what we keep)

1. **Semantic token pairs** — Voicebox defines `--primary`/`--card`/
   `--muted`/… in HSL triplets with `.dark` overrides; we keep our
   semantic set (bg/panel/inset/hover/text/muted/faint/border/accent/
   danger/warn/ok) and follow the same discipline. Adopted.
2. **Radius scale** — Voicebox derives `--radius-sm/md/lg` from one
   `--radius`. We already have `--radius: 8px` + `--radius-sm: 5px`.
   Adopted as-is; new components use the scale, never raw px.
3. **One elevation, low** — Voicebox's shadows are `0 1px 3px` class.
   Same as ours (`--shadow`). Panels are separated by 1 px borders, not
   shadows. Keep.
4. **Status colour discipline** — Voicebox's chip system (pass/warn/fail
   soft-background chips) matches our `.chip.pass/.warn/.fail`. Keep.
5. **Window chrome** — Voicebox runs a 1200×800 overlay titlebar
   (`titleBarStyle: Overlay`). Ours is already a custom titlebar; if
   resizing feels off, benchmark against these numbers. Reference only.
6. **Not adopted**: the amber accent, loaders.css spinner set, and the
   marketing-landing split (separate `landing/` site) — out of scope for
   a local-first desktop tool.

## Imagery & presentation (README / release assets)

Voicebox's presentation is a large part of why the project reads as
polished. The transferable pattern:

- **Centered hero**: icon (120 px) → title → one-line tagline → badge row.
- **Badges**: release version, CI status, licence, platform — flat
  `style=flat` shields, blue-grey, no clutter.
- **Screenshots**: full-width app screenshots in the README, alternating
  with prose. For ResearchAI this means: three-panel workspace, evidence
  matrix, setup checklist (capture when the app is running on real
  hardware; placeholder section reserved in README).
- **Dark + light variants of the icon** exist in Voicebox
  (`.github/assets/icon-dark.webp`). We ship one icon
  (`apps/desktop/src-tauri/icons/icon.png`) — fine for both themes since
  the mark sits on a neutral plate.

## Component rules (for new UI)

- Focus: `outline: 2px solid var(--accent); outline-offset: 1px;` on
  `:focus-visible` — already the convention in `app.css`; apply it
  everywhere, never `outline: none` without a replacement.
- Type: 14 px body / 15 px h2 / 20 px h1; `-0.01em` tracking on headings;
  monospace for IDs, paths, hashes, telemetry.
- Density: 24–28 px view padding, 6–10 px intra-group gaps, max content
  width 1040 px (`.view`).
- Chips carry status, never colour alone — pair with text label.
- Every long-running action shows determinate or at least textual
  progress (matches live export progress, streaming ask deltas).

## Related

- `.agents/skills/` — release + engine skills (Voicebox-derived).
- `docs/OPERATIONS.md` — how to run/verify the app that this design
  lives in.
