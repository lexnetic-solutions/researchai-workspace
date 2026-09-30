import { useCallback, useEffect, useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import type { DocumentSummary, NarrationKind, NarrationResult, SttStatus, TtsStatus } from '@researchai/shared-types';
import { backend } from '../backend/client';
import { EmptyState } from '../components/EmptyState';
import { useStore } from '../state/store';

function mmss(ms: number): string {
  const total = Math.round(ms / 1000);
  return `${String(Math.floor(total / 60)).padStart(2, '0')}:${String(total % 60).padStart(2, '0')}`;
}

const NARRATION_LABEL: Record<NarrationKind, string> = {
  read_aloud: 'Read aloud (document text, no AI)',
  summary_5: '5-minute summary',
  summary_10: '10-minute summary',
  summary_20: '20-minute summary',
  podcast: 'Podcast segment (two hosts)',
};

function formatBytes(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)} MB`;
  if (n >= 1_000) return `${Math.round(n / 1_000)} KB`;
  return `${n} B`;
}

function StatusDot({ ok, warn }: { ok: boolean; warn?: string }) {
  return (
    <span
      className={`chip ${ok ? 'status-ready' : 'status-failed'}`}
      title={ok ? 'Found' : (warn ?? 'Not found')}
    >
      {ok ? '✓' : '✕'}
    </span>
  );
}

export function AudioView() {
  const { activeProject, pushToast, native, setView } = useStore();
  const [status, setStatus] = useState<SttStatus | null>(null);
  const [tts, setTts] = useState<TtsStatus | null>(null);
  const [audioPath, setAudioPath] = useState('');
  const [transcribing, setTranscribing] = useState(false);
  const [result, setResult] = useState<{
    documentId: string;
    segments: number;
    durationMs: number;
    language: string | null;
    detail: string;
  } | null>(null);
  const [transcriptText, setTranscriptText] = useState('');
  // Speak section (Phase 8)
  const [docs, setDocs] = useState<DocumentSummary[]>([]);
  const [speakDocId, setSpeakDocId] = useState('');
  const [narrationKind, setNarrationKind] = useState<NarrationKind>('read_aloud');
  const [speaking, setSpeaking] = useState(false);
  const [narration, setNarration] = useState<NarrationResult | null>(null);

  const reloadStatus = useCallback(async () => {
    try {
      setStatus(await backend.sttCheck());
    } catch {
      setStatus(null);
    }
  }, []);

  const reloadTts = useCallback(async () => {
    try {
      setTts(await backend.ttsCheck());
    } catch {
      setTts(null);
    }
  }, []);

  const reloadDocs = useCallback(async () => {
    if (!activeProject) {
      setDocs([]);
      return;
    }
    try {
      const list = await backend.listDocuments(activeProject.id);
      setDocs(list);
      setSpeakDocId((prev) =>
        prev && list.some((d) => d.id === prev)
          ? prev
          : (list.find((d) => d.indexingStatus === 'ready')?.id ?? ''),
      );
    } catch {
      setDocs([]);
    }
  }, [activeProject]);

  useEffect(() => {
    void reloadStatus();
    void reloadTts();
  }, [reloadStatus, reloadTts]);

  useEffect(() => {
    void reloadDocs();
  }, [reloadDocs]);

  async function pickAudio() {
    if (native) {
      // No-filter single-file picker (the document picker's allow-list has no
      // audio types); the same command serves GGUF model selection.
      const picked = await backend.aiPickModelFile();
      if (picked) setAudioPath(picked);
      return;
    }
    const picked = window.prompt(
      'Browser preview: type an audio file path to simulate selection (cancel to abort).',
      '/Users/you/Recordings/lecture-01.wav',
    );
    if (picked) setAudioPath(picked);
  }

  async function transcribe() {
    if (!activeProject || !audioPath) return;
    setTranscribing(true);
    setResult(null);
    setTranscriptText('');
    try {
      const r = await backend.sttTranscribe(activeProject.id, audioPath);
      setResult(r);
      pushToast(
        'success',
        `Transcribed ${r.segments} segments (${mmss(r.durationMs)}). Saved to the library.`,
      );
      try {
        setTranscriptText(await backend.getDocumentText(r.documentId));
      } catch {
        /* preview is optional */
      }
      await reloadDocs();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setTranscribing(false);
    }
  }

  async function speak() {
    if (!speakDocId) return;
    setSpeaking(true);
    setNarration(null);
    try {
      const r =
        narrationKind === 'read_aloud'
          ? await backend.ttsSpeakDocument(speakDocId)
          : await backend.ttsNarrate(speakDocId, narrationKind);
      setNarration(r);
      pushToast('success', `Rendered ${mmss(r.durationMs)} of audio (${r.format.toUpperCase()}).`);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setSpeaking(false);
    }
  }

  async function revealAudio(path: string) {
    try {
      if (native) {
        await backend.revealPath(path);
      } else {
        pushToast('info', `Preview mock — the desktop app reveals ${path} in Finder.`);
      }
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  if (!activeProject) {
    return (
      <section className="view">
        <header className="view-head">
          <h1>Audio</h1>
          <p className="view-sub">Select a project to transcribe its recordings.</p>
        </header>
        <EmptyState title="No active project" hint="Create or select a project first." />
      </section>
    );
  }

  const configured = status?.configured ?? false;

  return (
    <section className="view">
      <header className="view-head">
        <h1>Audio</h1>
        <p className="view-sub">
          Lecture transcription with local whisper.cpp — offline, private, and
          timestamped. Transcripts land in the Library and join hybrid search.
        </p>
      </header>

      <div className="card">
        <h2>Whisper engine</h2>
        {status ? (
          <div className="field-row stt-status" style={{ marginTop: '0.5rem' }}>
            <span className="tiny muted">
              <StatusDot ok={status.cliFound} warn="whisper-cli not found at the configured path" />{' '}
              whisper-cli
            </span>
            <span className="tiny muted">
              <StatusDot ok={status.modelFound} warn="GGML model not found at the configured path" />{' '}
              model
            </span>
            <span className="tiny muted">
              <StatusDot
                ok={status.ffmpegFound}
                warn="ffmpeg not on PATH — non-WAV inputs need it"
              />{' '}
              ffmpeg
            </span>
            <span className={`chip ${configured ? 'status-ready' : 'status-waiting'}`}>
              {configured ? 'configured' : 'not configured'}
            </span>
          </div>
        ) : (
          <p className="tiny muted">Checking…</p>
        )}
        {!configured && (
          <p className="tiny muted" style={{ marginTop: '0.5rem' }}>
            Point ResearchAI at your <code>whisper-cli</code> binary and a GGML model in{' '}
            <button type="button" className="link" onClick={() => setView('settings')}>
              Settings → Speech
            </button>
            .
          </p>
        )}
      </div>

      <div className="card">
        <h2>Transcribe a recording</h2>
        <div className="field-row" style={{ marginTop: '0.5rem' }}>
          <button type="button" className="btn ghost" onClick={() => void pickAudio()}>
            Choose audio file…
          </button>
          {audioPath && (
            <span className="tiny muted audio-path" title={audioPath}>
              {audioPath}
            </span>
          )}
        </div>
        <div className="field-row" style={{ marginTop: '0.75rem' }}>
          <button
            type="button"
            className="btn primary"
            disabled={!audioPath || transcribing || !configured}
            onClick={() => void transcribe()}
          >
            {transcribing ? 'Transcribing… (local, may take minutes)' : 'Transcribe'}
          </button>
          {!native && <span className="tiny muted">(preview creates a mock transcript)</span>}
        </div>
        <p className="tiny muted" style={{ marginTop: '0.5rem' }}>
          WAV (16 kHz mono) runs directly; other audio/video formats are converted with ffmpeg
          when available. Nothing leaves this machine.
        </p>

        {result && (
          <div className="transcript-result" style={{ marginTop: '0.75rem' }}>
            <div className="field-row">
              <span className="chip status-ready">saved to library</span>
              <span className="tiny muted">
                {result.segments} segments · {mmss(result.durationMs)}
                {result.language ? ` · ${result.language}` : ''}
              </span>
              <button type="button" className="btn ghost" onClick={() => setView('documents')}>
                Open in Library
              </button>
            </div>
            {result.detail && <p className="tiny muted">{result.detail}</p>}
            {transcriptText && (
              <pre className="transcript-preview">{transcriptText}</pre>
            )}
          </div>
        )}
      </div>

      <div className="card">
        <h2>Speak a document</h2>
        {tts ? (
          <div className="field-row stt-status" style={{ marginTop: '0.5rem' }}>
            <span className="tiny muted">
              <StatusDot
                ok={tts.binaryFound}
                warn={
                  tts.provider === 'macos-say'
                    ? 'The macOS say command is unavailable'
                    : 'Piper not found at the configured path'
                }
              />{' '}
              {tts.provider === 'macos-say' ? 'macOS say' : 'piper'}
            </span>
            {tts.provider !== 'macos-say' && (
              <span className="tiny muted">
                <StatusDot ok={tts.modelFound} warn="Voice model (.onnx) not found" /> voice
                model
              </span>
            )}
            <span className={`chip ${tts.ready ? 'status-ready' : 'status-waiting'}`}>
              {tts.ready ? 'ready' : 'not ready'}
            </span>
          </div>
        ) : (
          <p className="tiny muted">Checking…</p>
        )}

        <div className="field-row" style={{ marginTop: '0.5rem' }}>
          <select
            className="search-scope"
            value={speakDocId}
            aria-label="Document to speak"
            onChange={(e) => setSpeakDocId(e.target.value)}
          >
            <option value="">Choose a document…</option>
            {docs.map((d) => (
              <option key={d.id} value={d.id} disabled={d.indexingStatus !== 'ready'}>
                {(d.title ?? d.fileName).slice(0, 60)}
              </option>
            ))}
          </select>
          <select
            className="search-scope"
            value={narrationKind}
            aria-label="Narration kind"
            onChange={(e) => setNarrationKind(e.target.value as NarrationKind)}
          >
            {(Object.keys(NARRATION_LABEL) as NarrationKind[]).map((k) => (
              <option key={k} value={k}>
                {NARRATION_LABEL[k]}
              </option>
            ))}
          </select>
        </div>
        <div className="field-row" style={{ marginTop: '0.75rem' }}>
          <button
            type="button"
            className="btn primary"
            disabled={!speakDocId || speaking || !tts?.ready}
            onClick={() => void speak()}
          >
            {speaking ? 'Rendering audio…' : 'Render audio'}
          </button>
          {!native && <span className="tiny muted">(preview creates a mock audio file)</span>}
        </div>
        <p className="tiny muted" style={{ marginTop: '0.5rem' }}>
          Read-aloud reads the document's own words — no AI needed. Summaries and the podcast
          segment are written by your local model, then spoken by Piper (or the macOS voice).
          Files land in the exports folder.
        </p>            {narration && (
              <div className="transcript-result" style={{ marginTop: '0.75rem' }}>
                <div className="field-row">
                  <span className="chip status-ready">{narration.format.toUpperCase()}</span>
                  <span className="tiny muted">
                    {mmss(narration.durationMs)} · {narration.words.toLocaleString()} words ·{' '}
                    {formatBytes(narration.bytes)} · {narration.engine}
                  </span>
                </div>
                {native ? (
                  <audio
                    className="audio-player"
                    controls
                    preload="metadata"
                    src={convertFileSrc(narration.audioPath)}
                  />
                ) : (
                  <p className="tiny muted" style={{ marginTop: '0.5rem' }}>
                    In-app playback is available in the desktop app.
                  </p>
                )}
                <div className="field-row" style={{ marginTop: '0.5rem' }}>
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={() => void revealAudio(narration.audioPath)}
                    title={narration.audioPath}
                  >
                    Reveal in Finder
                  </button>
                </div>
              </div>
            )}
      </div>

      <div className="two-col">
        <div className="card">
          <h2>How it works</h2>
          <ul className="check-list">
            <li>whisper.cpp runs per job — no server, no idle unload</li>
            <li>Segments keep [mm:ss] markers for precise citations</li>
            <li>Transcripts are regular documents: searchable, askable, exportable</li>
            <li>Re-transcribing the same file reuses the existing transcript</li>
          </ul>
        </div>
        <div className="card">
          <h2>Voice output</h2>
          <ul className="check-list">
            <li>Piper: local neural voices (.onnx), spec default</li>
            <li>macOS say: zero-install fallback voice</li>
            <li>MP3 export via ffmpeg when enabled</li>
            <li>Nothing leaves this machine — synthesis is local</li>
          </ul>
        </div>
      </div>
    </section>
  );
}
