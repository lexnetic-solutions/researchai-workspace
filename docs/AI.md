# Local AI (Phase 3 — implemented)

Grounded question answering over the private library, powered by a local
llama.cpp runtime. No cloud calls exist anywhere in this pipeline: the model,
its weights and every prompt stay on the device (spec §34, §40, §47.13).

## Interface boundary

Everything AI goes through the `AiProvider` trait
([ai.rs](../apps/desktop/src-tauri/src/services/ai.rs), spec §47.7):

```rust
pub trait AiProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn complete(&self, req: &CompletionRequest) -> AppResult<CompletionOutput>;
    fn complete_stream(&self, req: &CompletionRequest,
                       on_delta: &mut dyn FnMut(&str)) -> AppResult<CompletionOutput>;
}
```

- `LlamaCppProvider` — speaks llama-server's OpenAI-compatible API
  (`/v1/chat/completions`, `/health`) on loopback. SSE streaming and usage
  parsing are covered by loopback-HTTP tests that run without llama.cpp.
- `EchoProvider` — deterministic offline fallback used in tests; its output
  is loudly labelled "not a real answer".

The Rust core never links llama.cpp: the runtime is a separate,
crash-isolated process (see below). Future cloud providers implement the same
trait and stay opt-in (spec §34).

## Model manager

[model_manager.rs](../apps/desktop/src-tauri/src/services/model_manager.rs)
owns everything GGUF:

- **Validation** — magic-byte check plus a header walk that extracts
  `general.parameter_count` (→ "3.8B") and `general.file_type`
  (→ quantisation label). Weights are never read.
- **Import in place** — copies the file into the managed `models/` directory
  (originals are never moved), hashing SHA-256 in the same pass.
- **Download** — streamed to `models/` with progress events
  (`ai://model-download`) and cooperative cancel; non-GGUF payloads are
  rejected and deleted.
- **Lifecycle** — statuses (`available` / `missing`) are reconciled with the
  filesystem on every library view; deleting a registry row can optionally
  delete the file.

Model rows live in `local_models`, analyses in `analyses` (migration 4 — see
[DATABASE.md](DATABASE.md)).

## LLM runtime supervisor

[llm_runtime.rs](../apps/desktop/src-tauri/src/services/llm_runtime.rs)
owns the `llama-server` child process:

- Spawns it with `--ctx-size`, threads, GPU layers and any user-supplied
  extra args on a free loopback port, streaming stdout/stderr to
  `logs/llama-server.log`.
- Health-polls `/health` until weights are loaded (fail-fast if the process
  dies, with the log tail in the error).
- Writes `models/llama-server.pid` and terminates stale servers from crashed
  previous runs at startup (pid file first, command-line sweep second).
- Auto-unloads the model after the configured idle timeout and on app exit
  (spec §43: the LLM unloads when idle).
- The supervisor thread is the sole owner of the child; the UI observes a
  state machine: `idle → loading → ready | failed`, plus `unloaded`.

## Grounded ask pipeline

[analysis.rs](../apps/desktop/src-tauri/src/services/analysis.rs) — the
answer can only be as honest as the evidence it cites:

1. **Evidence** — hybrid retrieval (Phase 2) for project/library scope, or
   the document's chunks in reading order for single-document scope.
2. **Prompt** — strict system preamble ("answer ONLY from the numbered
   evidence", no invented sources) plus a per-mode instruction:
   Chat / Research / Quick Read / Deep Analysis / Critical (spec §17.2).
3. **Generation** — provider call with user-tuned limits (max tokens,
   temperature).
4. **Audit** — citations `[n]` are parsed out of the answer; an answer with
   zero citations is flagged as ungrounded in the trace. Answer, evidence,
   trace (engine, model, tokens, citations, warnings) are persisted to
   `analyses` and re-openable from the Research view history.

## Frontend

- **Research view** — the Ask panel: five modes, scope selector, runtime
  banner with manual load/unload, answers with clickable `[n]` citation
  chips that jump to the evidence card, debug panel with the analysis trace,
  and per-project history.
- **Settings → Local AI** — model library (import / download / select /
  delete), llama-server path, context size, max tokens, temperature, GPU
  layers, idle-unload timeout, runtime load/unload.
- **No-AI mode** (spec §35) is a hard gate: with AI disabled the Ask panel
  and Settings section still render, but every AI command is refused by the
  core — the app remains fully usable without AI.

## Verification

| Check | Result |
| --- | --- |
| Provider SSE/JSON parsing against canned loopback servers | 5/5 |
| Model manager (GGUF validation, import, download, scan, delete) | 8/8 |
| LLM runtime supervisor (fake llama-server, stale sweep) | 4/4 |
| Grounded ask pipeline (modes, citations, persistence) | 8/8 |
| IPC contract locks (camelCase wire format, key-for-key) | 9/9 |
| Full Rust suite (incl. Phase 1/2 + live E2E) | 50/50 |

## Known limitations

- The first Ask blocks while weights load (up to minutes on 8 GB machines);
  streaming into the UI is wired at the provider level but not surfaced in
  the Ask panel yet.
- Generation is single-shot: multi-turn chat flattens history into the
  question rather than keeping provider-side conversation state.
- There is no llama-server binary bundled or downloaded automatically — the
  user points Settings at an installed `llama-server` (packaging ships it in
  a later phase, see [PACKAGING.md](PACKAGING.md)).
- Prompt-injection hardening for imported documents (treat chunk text as
  data, never as instructions) is a noted follow-up for the security pass.
