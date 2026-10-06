# OPERATIONS — running, watching, and shipping ResearchAI

Practical guide for day-to-day operations: dev environment, CI, releases,
dry-runs, logs, and engine management. For architecture see
[ARCHITECTURE.md](ARCHITECTURE.md); AI internals in [AI.md](AI.md); speech in
[SPEECH_SETUP.md](SPEECH_SETUP.md); packaging details in [PACKAGING.md](PACKAGING.md).

---

## 1. Local dev environment

### Document engine (Python, port 8737)

The FastAPI engine ([services/document-engine](../services/document-engine))
runs in a detached `screen` so it survives terminal and agent restarts:

```bash
screen -dmS researchai-engine bash -c 'cd services/document-engine && exec uv run uvicorn researchai_document_engine.app:app --host 127.0.0.1 --port 8737'
```

Check it: `curl http://127.0.0.1:8737/health` → `{"status":"ok",...}`.

Rules of engagement:

- Never double-spawn: the desktop app classifies a healthy port-8737 engine as
  `External` and leaves it alone (see `engine_runtime.rs`). A running screen is
  therefore also *how tests exercise their external-engine path*.
- The desktop Rust test
  `services::engine_runtime::tests::spawns_bundled_fake_sidecar_and_drop_kills_it`
  intentionally asserts `External` when the port answers. With the engine
  stopped it takes the spawn+kill path instead — both are valid; the test logs
  tell you which ran.
- Restarting the engine on macOS: `screen -X quit` may only HUP the `uv` wrapper;
  kill the uvicorn PID (`ps aux | grep uvicorn`) if health stays up.

### Desktop app

```bash
pnpm install
pnpm dev            # frontend + Tauri dev shell
```

Rust needs `export PATH="$HOME/.cargo/bin:$PATH"` on this machine. Frontend
checks: `cd apps/desktop && pnpm exec tsc --noEmit && pnpm build`.

### Long-running Rust test runs

Interactive `cargo test` can outlive a tool call. The reliable pattern is a
detached screen with a log file:

```bash
screen -dmS cargo-test bash /path/to/runner.sh   # script: cd src-tauri; exec > log 2>&1; cargo test; echo CARGO_EXIT=$?
```

Then poll the log for `CARGO_EXIT=`. Gotchas learned the hard way:

- A stale log from a previous run can fake a "done" result — delete it before
  launching. Check `screen -ls` for `Dead ???` sessions and `screen -wipe`.
- `cargo test` holds an exclusive lock on the build dir; concurrent runs queue
  and can time out rather than fail loudly.

---

## 2. CI

Three workflows in [.github/workflows](../.github/workflows):

| Workflow | Trigger | What it does |
|---|---|---|
| `ci.yml` | every push | TypeScript typecheck+tests, Python engine tests, desktop Rust tests on 4 OS matrix (macos-15, macos-15-intel, windows-latest, ubuntu-24.04) |
| `release.yml` | tag `v*` | Builds + attaches signed/ad-hoc installers for the same 4 targets |
| `release-dry-run.yml` | manual (`workflow_dispatch`) | Same 4-target build, uploads artifacts, attaches nothing — the rehearsal for releases |

Operations notes:

- **Run-level status lags.** `gh run list` can show `queued` long after jobs
  started. Query jobs directly:
  `gh api repos/OWNER/REPO/actions/runs/<id>/jobs --jq '.jobs[] | "\(.name): \(.status)/\(.conclusion // "-") (\(.id))"'`
- **Job logs:** `gh api repos/OWNER/REPO/actions/jobs/<id>/logs`.
- **Pushes cancel in-flight CI.** An auto-cancelled run means its jobs never
  ran — a test that has "always passed" may simply have never executed on that
  OS. The first runs to reach the desktop matrix surfaced real platform bugs
  (see §6).
- **Retired runner labels never start.** GitHub retired `macos-13` entirely
  (2025-12-04) — jobs requesting it sit `queued` forever with no runner, which
  was originally mistaken for free-runner queue lag. The matrix now uses
  `macos-15-intel` (GitHub's last x86_64 image, supported to 2027-08) and
  `macos-15`; `macos-14` retires 2026-11-02 with brownouts before then. Judge a
  run by its job-level conclusions, and treat a label GitHub has removed as a
  workflow bug, not a slow queue.
- Linux CI targets Ubuntu 24.04; the .deb baseline is glibc ≥ 2.38.

---

## 3. Releases

1. Verify `main` is green at job level (all 4 desktop targets).
2. Cut the tag: `git tag v0.1.x <sha> && git push origin v0.1.x`.
3. `release.yml` builds and attaches installers to the GitHub release.
   The release is **published directly** (not a draft), and its body is
   extracted from the stamped `CHANGELOG.md` section for the tag by
   `scripts/release/extract-notes.sh` — so stamp the changelog
   (`release-bump` skill) *before* tagging. A missing section falls back
   to a generic body (never an empty one) and the dry-run rehearses the
   extraction step.
4. Expected assets: `aarch64.dmg`, `x64.dmg`, `x64-setup.exe`,
   `amd64.AppImage`, `amd64.deb`, plus the updater artefacts
   (`*_aarch64.app.tar.gz`, `*_x64.app.tar.gz` and one `.sig` per
   updater artefact) and — from the final manifest job — `latest.json`,
   which installed apps poll for self-updates. All assets attach within
   the run (the Intel leg on `macos-15-intel` is slower than arm64 but no
   longer unbounded); `latest.json` attaches only after every leg is done
   and is skipped (job fails) if any signature is missing.
5. Watch asset attachment at job level; an asset attaching *after* you edit
   the release is normal — assets attach automatically.
6. Self-update gate: the release is only self-update-capable if the
   `TAURI_SIGNING_PRIVATE_KEY*` secrets are set (they sign every artefact;
   `ci.yml` asserts signatures on every push). Losing the key breaks
   updates permanently — see PACKAGING.md.

Tag discipline: tags are immutable history. If a release must include newer
commits, cut a **new tag** (v0.1.x+1); do not re-point the old one unless you
accept the draft-release dance (deleting a remote tag turns its release into an
untagged draft; `gh release edit --draft=false` re-publishes it).

---

## 4. Dry-runs

`release-dry-run.yml` is manual-only: run it from the Actions tab or

```bash
gh workflow run release-dry-run.yml
```

Same matrix and build steps as `release.yml`, artifacts instead of assets —
use it to validate packaging changes without burning a tag. It also
rehearses the updater path with an **ephemeral throwaway signing key**
(never the release key), asserts the updater artefacts (`.app.tar.gz`,
`.sig`) exist per leg, and builds `latest.json` in a final manifest job —
but cannot upload anything (`permissions: contents: read`). Remember that
artifact uploads and bundle staging can differ subtly from a real release
run (signed vs ad-hoc steps), so a green dry-run is necessary, not
sufficient.

Pitfall fixed in this repo: decorative `find … | head -n` under `set -e -o
pipefail` SIGPIPE-kills the step once a staging directory exceeds the head
count. Keep Verify steps free of pipe-into-head listings.

---

## 5. Logs

**Desktop app** writes to `<data_dir>/logs/` (append-only, never leaves the
machine — see [logging.rs](../apps/desktop/src-tauri/src/logging.rs)). The
in-app Diagnostics view shows the exact path (`logs/ under …`). With the app
identifier `app.researchai.workspace`, that resolves under the platform data
dir (`~/Library/Application Support/app.researchai.workspace/logs/` on macOS).
`llama-server.log` lives there too, capturing model-load failures and
inference telemetry (`researchai::ai` lines log mode, tokens, ms/token).

**Document engine** logs to the screen session; `screen -r researchai-engine`
to watch, `Ctrl-A D` to detach.

**CI** logs are GitHub-side only (job logs API above); no runner artifacts are
kept beyond uploaded artifacts.

---

## 6. Operational pitfalls learned so far

- **Zombie sidecars.** Killing a child without reaping leaves a zombie;
  liveness probes (`kill -0`) keep reporting it alive. The engine supervisor
  now kills *and waits* on every exit path.
- **dash printf.** Ubuntu's `/bin/sh` is dash; its `printf` lacks `\xHH`.
  Test fakes must not emit binary via shell printf — generate bytes in Rust
  and `cat` them.
- **Windows RST-on-close.** A one-shot TCP server that hard-closes can abort
  the client's read with os error 10053 before the response lands. Half-close
  (`shutdown(Write)`), flush, drain.
- **Windows and shebang scripts.** Windows can't exec `#!`-scripted fakes
  (os error 193). Gate live-server tests to Unix; keep process-shape tests
  cross-platform.
- **pnpm/action-setup@v4** hard-fails if `version` input conflicts with
  `packageManager` in package.json — omit the input.
- **PyInstaller spec paths** resolve relative to the spec dir; anchor to
  `SPECPATH`.
- **Set-but-empty secrets are not unset.** `APPLE_SIGNING_IDENTITY=""` reads as
  an explicit empty identity; gate signing steps on a non-empty check.
- **Windows COLLECT sidecar** output is a directory; copy the entry binary to
  `researchai-engine.exe` (see [collect-sidecar.mjs](../scripts/package/collect-sidecar.mjs)).
- **bundle.resources glob** fails `cargo build` when nothing matches — keep a
  committed placeholder under `src-tauri/packaging/resources/sidecar/`.
- **SIGPIPE under `set -e -o pipefail`** (dry-run Verify): avoid
  `find | head -n` decorative listings.

---

## 7. Known operational decisions pending

- **Intel runner cost/speed:** the x64 .dmg target now builds on
  `macos-15-intel` (4 vCPU, slower than arm64); drop the leg if release
  latency ever matters more than Intel coverage.
- **Apple signing:** code paths for signed builds are gated and ready; waiting
  on Developer ID secrets.
- **Real-hardware validation:** first clean-machine install + end-to-end pass
  on the arm64 .dmg (and one Windows install) is still outstanding.
