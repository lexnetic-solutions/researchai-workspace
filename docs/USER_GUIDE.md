# ResearchAI Workspace — User Guide

A private, offline-first research workspace: import papers, read and search
them, ask questions with traceable citations, build evidence matrices, export
academic artefacts, transcribe lectures and listen to your documents — all on
your own machine.

**Nothing leaves your computer.** There are no cloud services and no accounts.

---

## 1. Install

### macOS

1. Download `ResearchAI Workspace_0.1.1_aarch64.dmg` (Apple Silicon) or the
   `x86_64` DMG (Intel).
2. Open the DMG and drag **ResearchAI Workspace** into **Applications**.
3. First launch: right-click the app → **Open** → **Open** again. (The app is
   ad-hoc signed; macOS Gatekeeper asks once. On Windows, click
   **More info → Run anyway** on the SmartScreen prompt.)

### Windows

1. Download `ResearchAI Workspace_0.1.1_x64-setup.exe`.
2. Run it and follow the installer.
3. If Windows SmartScreen warns, choose **More info → Run anyway**.

The installer bundles everything: the local AI model (Qwen3), the document
engine (parsing/OCR/embeddings) and the voice engine. **No Python, no
internet, no extra setup is required** for the core workflow.

---

## 2. First launch — read the Setup checklist

On **Home**, the **Setup checklist** verifies every subsystem live:

| Row | Green means |
|---|---|
| **Document engine** | Imports will parse, OCR and index. |
| **Local AI** | Ask-AI works — a starter model ships in the installer (“loads on first use” is still green). |
| **Lecture transcription** | whisper.cpp is configured — recording transcription works. |
| **Voice output** | Documents can be spoken aloud with your chosen voice. |

Anything amber shows a **Fix** button that jumps straight to the right
Settings section. You can start working before everything is green — only
the matching feature is affected.

---

## 3. Create a project

1. Go to **Projects** → **New project**.
2. Name it (e.g. “Thesis — coastal adaptation”) and add an optional
   description.
3. It becomes the **active project** — shown at the bottom of the sidebar.
   Everything you import, ask and export belongs to it.

> **Tip:** use one project per piece of work (thesis, subject, review). You
> can switch any time from **Projects** or the Home project picker.

---

## 4. Import your documents

1. Open **Library**.
2. Drag files **or a whole folder** onto the window, or click **Import** and
   pick files / a folder.
3. Supported formats: **PDF, DOCX, PPTX, XLSX, EPUB, HTML, TXT, MD** —
   scanned PDFs are OCR’d locally.
4. Documents are **copied into the managed workspace** (your originals are
   untouched) and de-duplicated by checksum — importing twice never creates
   a second copy.
5. Watch each row move to *Ready*. A failed row has a **Retry** button and an
   explanation.

When a document is *Ready* it is parsed, chunked and indexed for both keyword
and semantic (vector) search.

---

## 5. Read, search and annotate

- **Library** → click a document to open the reader; the outline, sections
  and full text are navigable.
- **Search** (or the search box in the title bar, then Enter) runs **hybrid
  retrieval**: keyword (FTS5) + meaning (vectors), fused into one ranking.
  Scores are visible for debugging. Scope it to the active project, the
  whole library, or a single document.

---

## 6. Ask questions (Research AI)

1. Open **Research AI**.
2. Type a question and choose the scope (active project or one document).
3. Pick a mode:
   - **Quick read** — fast, short answer.
   - **Deep analysis** — fuller reasoning over more evidence.
4. The answer cites numbered evidence inline — hover/click a citation to see
   the exact source passage. Answers with no citations are flagged
   *ungrounded*.

**No-AI mode** (Settings → AI mode) turns this off for a fully
AI-free workspace; search, imports, exports, transcription and read-aloud
keep working.

The bundled starter model (Qwen3, ~0.6B) works out of the box on CPU. Bring
your own larger **GGUF** model any time: **Settings → Local AI → Add model**
(faster and better on bigger machines; the model loads on first use and
unloads when idle).

---

## 7. Evidence matrices

1. Open **Evidence**.
2. Enter the comparison question (e.g. “How do the authors define
   resilience?”).
3. Choose the scope and generate — you get a matrix with one row per
   document, the best-matching passage, and a relevance score.
4. Saved matrices stay under **Saved evidence tables** and can be exported
   (step 9).

---

## 8. Citations & bibliography

1. Open **Bibliography**.
2. Choose a style: **APA 7 · Harvard · Chicago**.
3. Fix missing metadata (title/year) inline — the reader flags incomplete
   references.

---

## 9. Export

1. Open **Exports**.
2. **New export** → pick a **kind**:
   - **Bibliography** — formatted reference list,
   - **AI analysis** — a saved answer from Research AI,
   - **Evidence matrix** — a saved comparison table.
3. Pick a **format**: **Markdown, DOCX, PDF, BibTeX, RIS** (BibTeX/RIS for
   the bibliography, for reference managers like Zotero).
4. Choose the citation style, give it a source item, click **Export**.
5. Files land in the managed **exports folder** — listed under
   **Exported files**, with **Show in Finder/Explorer** and **Delete**.

---

## 10. Transcribe lectures & recordings

1. One-time setup (**Settings → Speech**):
   - **whisper-cli** path — e.g. `/opt/homebrew/bin/whisper-cli`
     (macOS: `brew install whisper-cpp`).
   - **GGML model** path — e.g. `ggml-base.bin` (147 MB, downloaded once).
   - Language: auto-detect or one of English/German/French/Spanish/Chinese.
   - Optional **ffmpeg** (`brew install ffmpeg`) lets the app convert any
     recording format to WAV automatically.
2. **Audio → Transcribe a recording** → pick the audio file → **Transcribe**.
3. The transcript is saved into the Library as a real document — searchable,
   citable and exportable like any other document, with timestamps.

Everything runs offline; recordings never leave the machine.

---

## 11. Voice output — listen to your documents

**Settings → Speech → Voice output** offers three voices:

| Provider | Setup |
|---|---|
| **Master voice (F5-TTS)** | Pick a ~10–30 s reference recording of the voice to clone (**Browse…**). It is copied into the managed workspace so it survives source-file cleanup. Requires `uv` (`brew install uv`) — cached model, fully offline after first use. |
| **Piper** | `brew install piper` + a `.onnx` voice — recommended for long narrations (fast, local neural voices). |
| **macOS say** | Zero setup — the built-in system voice. |

If the selected voice isn’t usable, the app automatically falls back
(Piper → macOS say) so audio generation always works; the status line always
shows the voice that will actually speak.

**Speaking a document (Audio tab):**

1. Choose a document and a **narration kind**:
   - **Read aloud** — the document’s own words (no AI needed),
   - **5 / 10 / 20-minute summary** — spoken summaries (uses the local AI),
   - **Podcast segment** — two hosts, conversational (uses the local AI).
2. Click **Render**. Long documents render in checkpointed batches — if
   anything interrupts the render, rerunning it resumes from the last
   completed batch instead of starting over.
3. Play the result inline, **Reveal** it in the file manager, or convert it
   to **MP3**.

**Voice test:** the one-sentence sample button renders immediately so you can
hear the configured chain before committing to a long document.

**MP3 conversion:** every render can be transcoded to MP3 (keeps the WAV), or
use **Convert audio to MP3** to convert *any* audio file beside the original.
This needs **ffmpeg** installed.

---

## 12. Settings & diagnostics (quick tour)

- **Appearance** — light / dark / system theme.
- **AI mode** — enable/disable AI, choose the model, context size, temperature,
  threads, GPU layers.
- **Storage** — managed data folder (projects, models, exports, database).
- **System diagnostics** — run a full subsystem check (engine, AI, speech,
  ffmpeg) with copyable results.
- **Speech** — transcription (whisper) + voice output (above).

**Where your data lives** (macOS):  
`~/Library/Application Support/app.researchai.workspace/`  
(`research.db`, `exports/`, `voices/`, `models/`, `logs/`). Back this folder
up to archive a project.

---

## 13. Troubleshooting

| Symptom | Fix |
|---|---|
| A checklist row is amber | Click its **Fix** button — it opens the right Settings section. |
| Imports stay *Failed* | Home → Document engine row: the bundled sidecar starts itself; retry from the failed row or **Settings → Diagnostics**. |
| Ask-AI is slow or weak | Settings → Local AI: add a bigger GGUF (e.g. Qwen3 4B/8B Q4) and set GPU layers if you have a discrete GPU. |
| Transcription button disabled | Settings → Speech: set whisper-cli + GGML model paths (row turns green when both files exist). |
| Voice render falls back to “macOS say” | The selected voice isn’t ready — check `uv` is installed and the reference recording exists (Settings → Speech). |
| MP3 buttons say “ffmpeg missing” | `brew install ffmpeg` (macOS) / install ffmpeg (Windows), restart the app. |
| Very long renders | Renders run batch-by-batch with checkpoints on disk; a crash or quit mid-render loses at most the current batch — just render again to resume. On Apple Silicon a rare GPU (MPS) crash automatically retries on CPU. |
| Something looks broken | **Settings → System diagnostics → Run diagnostics** and check `logs/researchai-YYYY-MM-DD.log` in the data folder. |

---

## 14. Good to know

- **Fully offline** after install (the only optional downloads are a bigger
  AI model or the first F5-TTS voice-model cache).
- **Projects are isolated** — deleting one never touches your originals.
- **No telemetry.**
- Keyboard: `Cmd/Ctrl+K` opens the command palette; the title-bar search box
  jumps straight to results.
