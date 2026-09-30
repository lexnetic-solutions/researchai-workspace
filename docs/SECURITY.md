# Security & Privacy

ResearchAI is private by construction. The threat model is ordinary user data
on a personal machine — the priorities are local-first storage, no silent
exfiltration, and safe handling of untrusted imported files.

## Storage

- Everything lives under the resolved app-data directory (`ResearchAIData`
  layout, spec §13): documents, database, logs, later models/exports.
- No telemetry, no crash reporting, no analytics. Logs are local files under
  `logs/`.
- Deleting the data directory (or a future in-app purge) removes all research
  data. Cloud sync/accounts do not exist in V1 (spec §40).

## Network posture

- The document engine binds to `127.0.0.1` only and makes no outbound calls.
- Phase 2: embeddings go to the loopback sidecar only.
- Phase 3: the only outbound call the Rust core can make is a user-initiated
  GGUF **model download** (explicit URL in Settings). `llama-server` runs on
  a free loopback port, spawned by the core; model weights and prompts never
  leave the machine. Cloud AI remains explicit opt-in with keys in the OS
  keychain (spec §34); the core never requires an API key (spec §47.13).

## Untrusted input

- Imported files are **never executed**. Parsing happens in the Python
  sidecar; the Rust core only hashes/validates file types
  ([ingestion.rs](../apps/desktop/src-tauri/src/services/ingestion.rs)).
  GGUF model files are validated by header sniffing before registration and
  are data only — never executed by the core (llama-server reads them, as a
  user-supplied binary the user chose to point the app at).
- AI answers are generated only from numbered excerpts of the user's own
  library ([AI.md](AI.md)); answers without citations are flagged in the
  trace. Prompt-injection hardening (treating document text strictly as
  data) is a noted follow-up for the security pass.
- Path handling: sources come from the OS file picker, not from web content;
  no user string is ever joined into a base path blindly.
- Webview CSP blocks remote script; images restricted to self/data/asset
  ([tauri.conf.json](../apps/desktop/src-tauri/tauri.conf.json),
  [index.html](../apps/desktop/index.html)).
- Imported HTML is sanitised and remote content blocked by default once HTML
  rendering lands (Phase 1).

## Webview hardening

- Tauri capability file grants only `core:default`, `dialog:default`,
  `opener:default` ([capabilities/default.json](../apps/desktop/src-tauri/capabilities/default.json)).
- `opener` is used solely to reveal files in the OS file manager.

## Error handling (spec §42)

Background tasks are recoverable; failed documents are recorded per-file and
the queue continues. The UI shows human-readable messages; details go to the
local log — stack traces never reach end users.
