import { useCallback, useEffect, useState } from 'react';
import type { SttStatus, TtsStatus } from '@researchai/shared-types';
import { backend } from '../backend/client';
import { useStore } from '../state/store';
import { EmptyState } from '../components/EmptyState';

type CheckState = 'pending' | 'ok' | 'attention';

interface SetupCheck {
  readonly id: string;
  readonly label: string;
  readonly state: CheckState;
  readonly detail: string;
  readonly view?: 'library' | 'research' | 'audio' | 'settings';
}

/** First-run setup checklist (Phase 9, spec §48): live feature probes. */
function useSetupChecks(): SetupCheck[] {
  const { native } = useStore();
  const [engineUp, setEngineUp] = useState<boolean | null>(null);
  const [aiEnabled, setAiEnabled] = useState<boolean | null>(null);
  const [modelReady, setModelReady] = useState<boolean | null>(null);
  const [stt, setStt] = useState<SttStatus | null>(null);
  const [tts, setTts] = useState<TtsStatus | null>(null);

  const reload = useCallback(async () => {
    try {
      setEngineUp(await backend.probeDocumentEngine());
    } catch {
      setEngineUp(false);
    }
    try {
      const settings = await backend.getSettings();
      setAiEnabled(settings.aiEnabled);
      if (settings.aiEnabled) {
        try {
          const rt = await backend.aiRuntimeStatus();
          setModelReady(rt.state.state === 'ready');
        } catch {
          setModelReady(false);
        }
      }
    } catch {
      setAiEnabled(null);
    }
    try {
      setStt(await backend.sttCheck());
    } catch {
      setStt(null);
    }
    try {
      setTts(await backend.ttsCheck());
    } catch {
      setTts(null);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  return [
    {
      id: 'engine',
      label: 'Document engine (parsing, OCR, embeddings)',
      state: engineUp === null ? 'pending' : engineUp ? 'ok' : 'attention',
      detail:
        engineUp == null
          ? 'Checking…'
          : engineUp
            ? 'Online — imports will parse and index.'
            : native
              ? 'Offline — the bundled sidecar starts automatically; retry from Diagnostics if imports fail.'
              : 'Offline in browser preview (expected).',
      view: 'settings',
    },
    {
      id: 'ai',
      label: 'Local AI (Ask, evidence, narration scripts)',
      state: aiEnabled === null ? 'pending' : !aiEnabled ? 'attention' : modelReady ? 'ok' : 'attention',
      detail:
        aiEnabled == null
          ? 'Checking…'
          : !aiEnabled
            ? 'No-AI mode is on — the app works without it; enable AI for Ask & summaries.'
            : modelReady
              ? 'Model loaded and ready.'
              : 'Add a GGUF model in Settings → Local AI and load it.',
      view: 'settings',
    },
    {
      id: 'speech-in',
      label: 'Lecture transcription (whisper.cpp)',
      state: stt === null ? 'pending' : stt.configured ? (stt.cliFound && stt.modelFound ? 'ok' : 'attention') : 'attention',
      detail:
        stt == null
          ? 'Checking…'
          : stt.configured
            ? stt.cliFound && stt.modelFound
              ? 'whisper-cli and model found — ready to transcribe.'
              : 'Paths set but files missing on disk — re-check in Settings → Speech.'
            : 'Point ResearchAI at whisper-cli + a GGML model in Settings → Speech.',
      view: 'audio',
    },
    {
      id: 'speech-out',
      label: 'Voice output (Master voice / Piper / macOS say)',
      state: tts === null ? 'pending' : tts.ready ? 'ok' : 'attention',
      detail:
        tts == null
          ? 'Checking…'
          : tts.ready
            ? `Voice ready (${
                tts.provider === 'master-voice'
                  ? 'master voice (F5-TTS)'
                  : tts.provider === 'macos-say'
                    ? 'macOS say'
                    : 'Piper'
              }).`
            : 'Set a master voice recording, install piper, or use the macOS voice in Settings → Speech.',
      view: 'settings',
    },
  ];
}

export function HomeView() {
  const { projects, activeProject, setActiveProjectId, setView, native } = useStore();
  const checks = useSetupChecks();

  return (
    <section className="view">
      <header className="view-head">
        <h1>Home</h1>
        <p className="view-sub">
          A private, offline-first research workspace. Your documents stay on this
          machine — {native ? 'managed locally by the desktop app' : 'preview mode runs entirely in-browser'}.
        </p>
      </header>

      <div className="home-grid">
        <div className="card">
          <h2>Active project</h2>
          {activeProject ? (
            <>
              <p className="card-title">{activeProject.name}</p>
              {activeProject.description && <p className="muted">{activeProject.description}</p>}
              <button type="button" className="btn" onClick={() => setView('library')}>
                Open Library
              </button>
            </>
          ) : (
            <>
              <p className="muted">No project selected.</p>
              {projects.length > 0 ? (
                <ul className="project-picker">
                  {projects.map((p) => (
                    <li key={p.id}>
                      <button
                        type="button"
                        className="link"
                        onClick={() => {
                          setActiveProjectId(p.id);
                          setView('library');
                        }}
                      >
                        {p.name}
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <button type="button" className="btn" onClick={() => setView('projects')}>
                  Create your first project
                </button>
              )}
            </>
          )}
        </div>

        <div className="card">
          <h2>Start here</h2>
          <ol className="steps">
            <li>Create a project for your thesis, coursework or review.</li>
            <li>Import a folder of PDFs, DOCX, slides or notes.</li>
            <li>Read, search and annotate your library.</li>
            <li>Ask questions with traceable citations.</li>
            <li>Export summaries and evidence matrices.</li>
          </ol>
        </div>

        <div className="card">
          <h2>Workspace status</h2>
          <ul className="status-lines">
            <li>
              <span className="status-dot ok" /> Local storage ready
            </li>
            <li>
              <span className="status-dot ok" /> No cloud services configured
            </li>
          </ul>
          <button type="button" className="btn ghost" onClick={() => setView('settings')}>
            Open Settings &amp; Diagnostics
          </button>
        </div>
      </div>

      <div className="card wide">
        <h2>Setup checklist</h2>
        <ul className="setup-checklist">
          {checks.map((c) => (
            <li key={c.id} className="setup-check">
              <span className={`status-dot ${c.state === 'ok' ? 'ok' : c.state === 'attention' ? 'warn' : 'unknown'}`} />
              <div className="setup-check-body">
                <div>{c.label}</div>
                <div className="tiny muted">{c.detail}</div>
              </div>
              {c.view && c.state === 'attention' && (
                <button type="button" className="btn ghost tiny-btn" onClick={() => setView(c.view!)}>
                  Fix
                </button>
              )}
            </li>
          ))}
        </ul>
      </div>

      {projects.length === 0 && (
        <EmptyState
          title="No projects yet"
          hint="Projects group your documents, notes, evidence and exports."
        />
      )}
    </section>
  );
}
