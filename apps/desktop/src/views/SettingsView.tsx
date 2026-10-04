import { useCallback, useEffect, useState } from 'react';
import type {
  AiSettings,
  DiagnosticsReport,
  LocalModel,
  ModelDownloadEvent,
  RuntimeStatus,
  SttSettings,
  SttStatus,
  TtsSettings,
  TtsStatus,
} from '@researchai/shared-types';
import { backend, onModelDownload } from '../backend/client';
import { useStore, applyTheme } from '../state/store';

function formatBytes(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)} GB`;
  if (n >= 1_000_000) return `${Math.round(n / 1_000_000)} MB`;
  return `${n} B`;
}

function runtimeLabel(state: RuntimeStatus['state']): string {
  switch (state.state) {
    case 'idle':
      return 'Not loaded';
    case 'loading':
      return 'Loading…';
    case 'ready':
      return `Ready (port ${state.port})`;
    case 'failed':
      return 'Failed';
    case 'unloaded':
      return 'Unloaded';
  }
}

/** Local AI section: model library, generation settings and runtime. */
function LocalAiSection() {
  const { pushToast, native } = useStore();
  const [models, setModels] = useState<LocalModel[]>([]);
  const [ai, setAi] = useState<AiSettings | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [downloadUrl, setDownloadUrl] = useState('');
  const [download, setDownload] = useState<ModelDownloadEvent | null>(null);

  const reload = useCallback(async () => {
    try {
      setModels(await backend.aiListModels());
      setAi(await backend.aiGetSettings());
      setRuntime(await backend.aiRuntimeStatus());
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }, [pushToast]);

  useEffect(() => {
    void reload();
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onModelDownload((ev) => {
      setDownload(ev);
      if (ev.state !== 'running') {
        window.setTimeout(() => setDownload(null), 4000);
        void reload();
      }
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [reload]);

  async function pickAndAdd() {
    try {
      const path = native ? await backend.aiPickModelFile() : window.prompt('Path to .gguf file:');
      if (!path) return;
      setBusy(true);
      const model = await backend.aiAddModel(path);
      pushToast('success', `Added ${model.fileName}.`);
      await reload();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function startDownload() {
    if (!downloadUrl.trim()) return;
    try {
      await backend.aiDownloadModel(downloadUrl.trim());
      pushToast('info', 'Download started.');
      setDownloadUrl('');
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function chooseModel(id: string) {
    if (!ai) return;
    try {
      const next = { ...ai, activeModelId: id };
      setAi(await backend.aiSaveSettings(next));
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function deleteModel(id: string) {
    try {
      await backend.aiDeleteModel(id, true);
      await reload();
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function saveAi(next: AiSettings) {
    try {
      setAi(await backend.aiSaveSettings(next));
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  async function loadModel() {
    setBusy(true);
    try {
      const status = await backend.aiLoadModel();
      setRuntime(status);
      if (status.state.state === 'ready') pushToast('success', 'Local model is ready.');
      else if (status.state.state === 'failed') pushToast('error', status.state.detail);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
      void reload();
    } finally {
      setBusy(false);
    }
  }

  if (!ai) return null;
  const runtimeState = runtime?.state.state ?? 'idle';

  return (
    <div className="card">
      <h2>Local AI</h2>
      <p className="tiny muted">
        GGUF models run through llama.cpp (llama-server) on this machine only. No model or
        prompt ever leaves the device.
      </p>

      <div className="field-row" style={{ marginTop: '0.75rem' }}>
        <span className={`chip ${runtimeState === 'ready' ? 'pass' : runtimeState === 'failed' ? 'fail' : 'subtle'}`}>
          {runtimeLabel(runtime?.state ?? { state: 'idle' })}
        </span>
        <button
          type="button"
          className="btn tiny-btn"
          disabled={busy || runtimeState === 'ready' || runtimeState === 'loading'}
          onClick={() => void loadModel()}
        >
          {runtimeState === 'loading' ? 'Loading…' : 'Load selected model'}
        </button>
        <button
          type="button"
          className="btn ghost tiny-btn"
          disabled={runtimeState !== 'ready'}
          onClick={async () => setRuntime(await backend.aiUnloadModel())}
        >
          Unload
        </button>
        {!native && <span className="tiny muted">(preview mock)</span>}
      </div>

      <h3 className="evidence-title">Model library</h3>
      {models.length === 0 ? (
        <p className="tiny muted">
          No models yet — import a .gguf file or download one (e.g. a Qwen3 or Llama 3.x
          small-instruct quant from Hugging Face).
        </p>
      ) : (
        <table className="diag-table">
          <thead>
            <tr>
              <th>Model</th>
              <th>Size</th>
              <th>Status</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {models.map((m) => (
              <tr key={m.id} className={ai.activeModelId === m.id ? 'active-row' : ''}>
                <td>
                  <label className="model-row">
                    <input
                      type="radio"
                      name="active-model"
                      checked={ai.activeModelId === m.id}
                      onChange={() => void chooseModel(m.id)}
                    />
                    <span>
                      {m.fileName}
                      <span className="tiny muted">
                        {' '}
                        · {m.parameters ?? '?'} · {m.quantization ?? '?'}
                      </span>
                    </span>
                  </label>
                </td>
                <td>{formatBytes(m.sizeBytes)}</td>
                <td>
                  <span className={`chip ${m.status === 'available' ? 'pass' : 'fail'}`}>{m.status}</span>
                </td>
                <td>
                  <button
                    type="button"
                    className="btn ghost tiny-btn"
                    onClick={() => void deleteModel(m.id)}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <div className="field-row" style={{ marginTop: '0.75rem' }}>
        <button type="button" className="btn" disabled={busy} onClick={() => void pickAndAdd()}>
          Import .gguf file…
        </button>
      </div>
      <div className="field-row" style={{ marginTop: '0.5rem' }}>
        <input
          className="search-input"
          style={{ flex: 1 }}
          placeholder="…or paste a direct .gguf download URL"
          value={downloadUrl}
          onChange={(e) => setDownloadUrl(e.target.value)}
          aria-label="Download URL"
        />
        <button type="button" className="btn ghost" disabled={!downloadUrl.trim()} onClick={() => void startDownload()}>
          Download
        </button>
      </div>
      {download && (
        <div className="tiny muted" style={{ marginTop: '0.25rem' }}>
          {download.state === 'running'
            ? `Downloading ${download.fileName}: ${formatBytes(download.downloadedBytes)}${download.totalBytes ? ` / ${formatBytes(download.totalBytes)}` : ''}`
            : `Download ${download.state}${download.error ? `: ${download.error}` : ''}`}
        </div>
      )}

      <h3 className="evidence-title">Generation & runtime</h3>
      <div className="ai-settings-grid">
        <label>
          <span className="diag-label">llama-server path</span>
          <input
            className="search-input"
            value={ai.llamaServerPath}
            placeholder="/opt/homebrew/bin/llama-server"
            onChange={(e) => setAi({ ...ai, llamaServerPath: e.target.value })}
            onBlur={() => void saveAi(ai)}
          />
        </label>
        <label>
          <span className="diag-label">Context size</span>
          <input
            type="number"
            min={512}
            max={131072}
            step={512}
            className="search-input"
            value={ai.contextSize}
            onChange={(e) => setAi({ ...ai, contextSize: Number(e.target.value) || 4096 })}
            onBlur={() => void saveAi(ai)}
          />
        </label>
        <label>
          <span className="diag-label">Max answer tokens</span>
          <input
            type="number"
            min={16}
            max={8192}
            className="search-input"
            value={ai.maxTokens}
            onChange={(e) => setAi({ ...ai, maxTokens: Number(e.target.value) || 1024 })}
            onBlur={() => void saveAi(ai)}
          />
        </label>
        <label>
          <span className="diag-label">Temperature</span>
          <input
            type="number"
            min={0}
            max={2}
            step={0.1}
            className="search-input"
            value={ai.temperature}
            onChange={(e) => setAi({ ...ai, temperature: Number(e.target.value) })}
            onBlur={() => void saveAi(ai)}
          />
        </label>
        <label>
          <span className="diag-label">GPU layers</span>
          <input
            type="number"
            min={0}
            max={100}
            className="search-input"
            value={ai.gpuLayers}
            onChange={(e) => setAi({ ...ai, gpuLayers: Number(e.target.value) || 0 })}
            onBlur={() => void saveAi(ai)}
          />
        </label>
        <label>
          <span className="diag-label">Idle unload (min)</span>
          <input
            type="number"
            min={0}
            max={120}
            className="search-input"
            value={ai.idleUnloadMinutes}
            onChange={(e) => setAi({ ...ai, idleUnloadMinutes: Number(e.target.value) || 0 })}
            onBlur={() => void saveAi(ai)}
          />
        </label>
      </div>
      <p className="tiny muted">
        Generation settings apply to the next question. Hardware profile drives what fits —
        keep the context small on 8 GB machines (spec §36/§43).
      </p>
    </div>
  );
}

export function SettingsView() {
  const { settings, setSettings, pushToast, native } = useStore();
  const [report, setReport] = useState<DiagnosticsReport | null>(null);
  const [running, setRunning] = useState(false);

  const refresh = useCallback(async () => {
    setRunning(true);
    try {
      setReport(await backend.runDiagnostics());
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setRunning(false);
    }
  }, [pushToast]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function changeTheme(theme: 'light' | 'dark' | 'system') {
    try {
      const updated = await backend.setTheme(theme);
      setSettings(updated);
      applyTheme(updated.theme);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <section className="view">
      <header className="view-head">
        <h1>Settings</h1>
        <p className="view-sub">Local-first configuration. No accounts, no cloud.</p>
      </header>

      <div className="two-col">
        <div className="card">
          <h2>Appearance</h2>
          <div className="field-row">
            {(['light', 'dark', 'system'] as const).map((t) => (
              <button
                key={t}
                type="button"
                className={`btn ${settings?.theme === t ? 'primary' : 'ghost'}`}
                onClick={() => void changeTheme(t)}
              >
                {t[0].toUpperCase() + t.slice(1)}
              </button>
            ))}
          </div>
        </div>

        <div className="card">
          <h2>AI mode</h2>
          <div className="field-row">
            <button
              type="button"
              className={`btn ${settings?.aiEnabled ? 'ghost' : 'primary'}`}
              onClick={async () => setSettings(await backend.setAiEnabled(false))}
            >
              No-AI (offline)
            </button>
            <button
              type="button"
              className={`btn ${settings?.aiEnabled ? 'primary' : 'ghost'}`}
              onClick={async () => setSettings(await backend.setAiEnabled(true))}
            >
              AI enabled
            </button>
          </div>
          <p className="tiny muted">
            No-AI mode hides every AI feature; the app stays fully usable (spec §35). Enabling AI
            is local-only — models run on this machine and never call the cloud.
          </p>
        </div>

        <div className="card">
          <h2>Storage</h2>
          <p className="mono tiny">{settings?.dataDirectory ?? '…'}</p>
          <p className="tiny muted">
            All documents, notes, the database and future models live under this folder.
            Deleting it removes all research data (spec §34).
          </p>
          <p className="tiny muted">
            Default import mode: <strong>{settings?.defaultImportMode ?? 'managed-copy'}</strong>{' '}
            (managed copy keeps projects portable).
          </p>
        </div>
      </div>

      <div className="card">
        <h2>System diagnostics</h2>
        {!native && (
          <p className="tiny warn-text">
            Browser preview: hardware numbers come from browser hints, not the OS.
          </p>
        )}
        {report && (
          <>
            <div className="diag-grid">
              <div>
                <div className="diag-label">OS</div>
                <div>{report.system.osName} {report.system.osVersion}</div>
              </div>
              <div>
                <div className="diag-label">CPU</div>
                <div>{report.system.cpu.name} ({report.system.cpu.cores} cores)</div>
              </div>
              <div>
                <div className="diag-label">Memory</div>
                <div>
                  {Math.round(report.system.totalMemoryMb / 1024)} GB total ·{' '}
                  {Math.round(report.system.availableMemoryMb / 1024)} GB available
                </div>
              </div>
              <div>
                <div className="diag-label">Hardware profile</div>
                <div>
                  <span className="chip profile">{report.system.profile.toUpperCase()}</span>
                  <span className="tiny muted"> drives model size &amp; concurrency defaults</span>
                </div>
              </div>
              <div>
                <div className="diag-label">Database</div>
                <div>{report.databaseOk ? 'OK' : 'Failed'}</div>
              </div>
              <div>
                <div className="diag-label">Document engine</div>
                <div>{report.documentEngineOk ? 'Online' : 'Offline (expected until sidecar starts)'}</div>
              </div>
            </div>
            <table className="diag-table">
              <thead>
                <tr>
                  <th>Check</th>
                  <th>Status</th>
                  <th>Detail</th>
                </tr>
              </thead>
              <tbody>
                {report.checks.map((c) => (
                  <tr key={c.id}>
                    <td>{c.label}</td>
                    <td>
                      <span className={`chip ${c.status}`}>{c.status}</span>
                    </td>
                    <td className="muted">{c.detail}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
        <button type="button" className="btn" disabled={running} onClick={() => void refresh()}>
          {running ? 'Running…' : 'Re-run diagnostics'}
        </button>
      </div>

      <LocalAiSection />
      <SpeechSection />
    </section>
  );
}

/** Speech-to-text section (Phase 7): local whisper.cpp configuration. */
function SpeechSection() {
  const { pushToast, native } = useStore();
  const [stt, setStt] = useState<SttSettings | null>(null);
  const [status, setStatus] = useState<SttStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [tts, setTts] = useState<TtsSettings | null>(null);
  const [ttsStatus, setTtsStatus] = useState<TtsStatus | null>(null);
  const [ttsBusy, setTtsBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const [s, st] = await Promise.all([backend.sttGetSettings(), backend.sttCheck()]);
      setStt(s);
      setStatus(st);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
    try {
      const [t, tst] = await Promise.all([backend.ttsGetSettings(), backend.ttsCheck()]);
      setTts(t);
      setTtsStatus(tst);
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    }
  }, [pushToast]);

  useEffect(() => {
    void reload();
  }, [reload]);

  async function save(next: SttSettings) {
    setBusy(true);
    try {
      setStt(await backend.sttSaveSettings(next));
      setStatus(await backend.sttCheck());
      pushToast('success', 'Speech settings saved.');
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function pickPath(field: 'whisperCliPath' | 'whisperModelPath', kind: string) {
    if (native) {
      const picked = await backend.aiPickModelFile();
      if (picked && stt) void save({ ...stt, [field]: picked });
      return;
    }
    const picked = window.prompt(
      `Browser preview: type the ${kind} path (cancel to abort).`,
      field === 'whisperCliPath'
        ? '/opt/whisper.cpp/build/bin/whisper-cli'
        : '/opt/whisper.cpp/models/ggml-base.bin',
    );
    if (picked && stt) void save({ ...stt, [field]: picked });
  }

  async function pickTtsPath(
    field: 'piperPath' | 'voiceModelPath' | 'masterRefPath',
    kind: string,
  ) {
    if (native) {
      const picked = await backend.aiPickModelFile();
      if (picked && tts) void saveTts({ ...tts, [field]: picked });
      return;
    }
    const picked = window.prompt(
      `Browser preview: type the ${kind} path (cancel to abort).`,
      field === 'piperPath'
        ? '/opt/homebrew/bin/piper'
        : field === 'masterRefPath'
          ? '/Users/you/Recordings/voice-sample.wav'
          : '/opt/piper/voices/en_US-amy-medium.onnx',
    );
    if (picked && tts) void saveTts({ ...tts, [field]: picked });
  }

  async function saveTts(next: TtsSettings) {
    setTtsBusy(true);
    try {
      const saved = await backend.ttsSaveSettings(next);
      setTts(saved);
      setTtsStatus(await backend.ttsCheck());
      pushToast('success', 'Voice settings saved.');
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setTtsBusy(false);
    }
  }

  if (!stt) return null;

  return (
    <div className="card">
      <h2>Speech</h2>
      {/* stt + tts settings render below; tts section is guarded on tts load */}
      <p className="tiny muted">
        Lecture transcription runs through your local whisper.cpp build — the app never
        bundles or downloads models. Point it at a <code>whisper-cli</code> binary and a GGML
        model (e.g. <code>ggml-base.bin</code>); larger models are more accurate but slower.
      </p>

      {status && (
        <div className="field-row" style={{ marginTop: '0.5rem' }}>
          <span className={`chip ${status.cliFound ? 'pass' : 'fail'}`}>
            whisper-cli {status.cliFound ? 'found' : 'missing'}
          </span>
          <span className={`chip ${status.modelFound ? 'pass' : 'fail'}`}>
            model {status.modelFound ? 'found' : 'missing'}
          </span>
          <span className={`chip ${status.ffmpegFound ? 'pass' : 'subtle'}`}>
            ffmpeg {status.ffmpegFound ? 'on PATH' : 'not on PATH'}
          </span>
          {!native && <span className="tiny muted">(preview: paths are mocked)</span>}
        </div>
      )}

      <div className="field-row" style={{ marginTop: '0.75rem' }}>
        <input
          className="search-input"
          value={stt.whisperCliPath}
          placeholder="/path/to/whisper-cli"
          aria-label="whisper-cli binary path"
          onChange={(e) => setStt({ ...stt, whisperCliPath: e.target.value })}
        />
        <button
          type="button"
          className="btn ghost tiny-btn"
          onClick={() => void pickPath('whisperCliPath', 'whisper-cli binary')}
        >
          Browse…
        </button>
      </div>
      <div className="field-row" style={{ marginTop: '0.5rem' }}>
        <input
          className="search-input"
          value={stt.whisperModelPath}
          placeholder="/path/to/ggml-base.bin"
          aria-label="whisper GGML model path"
          onChange={(e) => setStt({ ...stt, whisperModelPath: e.target.value })}
        />
        <button
          type="button"
          className="btn ghost tiny-btn"
          onClick={() => void pickPath('whisperModelPath', 'GGML model')}
        >
          Browse…
        </button>
      </div>

      <div className="field-row" style={{ marginTop: '0.5rem' }}>
        <select
          className="search-scope"
          value={stt.language}
          aria-label="Transcription language"
          onChange={(e) => setStt({ ...stt, language: e.target.value })}
        >
          <option value="auto">Detect language</option>
          <option value="en">English</option>
          <option value="de">German</option>
          <option value="fr">French</option>
          <option value="es">Spanish</option>
          <option value="zh">Chinese</option>
        </select>
        <label className="model-row tiny">
          <input
            type="checkbox"
            checked={stt.convertWithFfmpeg}
            onChange={(e) => setStt({ ...stt, convertWithFfmpeg: e.target.checked })}
          />
          Convert non-WAV input with ffmpeg
        </label>
      </div>

      <div className="field-row" style={{ marginTop: '0.75rem' }}>
        <button
          type="button"
          className="btn primary tiny-btn"
          disabled={busy}
          onClick={() => void save(stt)}
        >
          {busy ? 'Saving…' : 'Save speech settings'}
        </button>
      </div>
      <p className="tiny muted" style={{ marginTop: '0.5rem' }}>
        16 kHz mono WAV transcribes directly; anything else converts via ffmpeg first (install
        with <code>brew install ffmpeg</code> if needed).
      </p>

      <h3 className="evidence-title" style={{ marginTop: '1rem' }}>
        Voice output (text-to-speech)
      </h3>
      {tts && ttsStatus && (
        <>
          <div className="field-row" style={{ marginTop: '0.5rem' }}>
            <select
              className="search-scope"
              value={tts.provider}
              aria-label="Voice provider"
              onChange={(e) => setTts({ ...tts, provider: e.target.value })}
            >
              <option value="master-voice">
                Master voice (clone your own recording, offline)
              </option>
              <option value="piper">Piper (recommended, local neural voices)</option>
              <option value="macos-say">macOS say (built-in voice)</option>
            </select>
            <span
              className={`chip ${ttsStatus.ready ? 'pass' : 'fail'}`}
              title={
                ttsStatus.provider === 'master-voice'
                  ? 'F5-TTS clones the reference recording, entirely on this machine'
                  : ttsStatus.provider === 'macos-say'
                    ? 'Uses the built-in macOS speech service'
                    : 'Needs the piper binary and a voice model below'
              }
            >
              {ttsStatus.ready ? 'ready' : 'not ready'}
            </span>
            {tts.provider === 'master-voice' && (
              <span
                className={`chip ${ttsStatus.masterAssetsCached ? 'pass' : 'subtle'}`}
                title={
                  ttsStatus.masterAssetsCached
                    ? 'F5-TTS model is cached — renders run fully offline'
                    : 'First render downloads the F5-TTS model (~1.3 GB), then runs offline'
                }
              >
                {ttsStatus.masterAssetsCached
                  ? 'F5 model cached'
                  : 'F5 model downloads on first render'}
              </span>
            )}
          </div>

          {tts.provider === 'master-voice' ? (
            <>
              <div className="field-row" style={{ marginTop: '0.5rem' }}>
                <input
                  className="search-input"
                  value={tts.masterRefPath}
                  placeholder="/path/to/voice-sample.wav"
                  aria-label="Master voice reference recording"
                  onChange={(e) => setTts({ ...tts, masterRefPath: e.target.value })}
                />
                <button
                  type="button"
                  className="btn ghost tiny-btn"
                  onClick={() => void pickTtsPath('masterRefPath', 'master voice recording')}
                >
                  Browse…
                </button>
              </div>
              <p className="tiny muted" style={{ marginTop: '0.35rem' }}>
                Pick a clean recording of the voice to clone (10–30 s of solo speech is ideal;
                the app copies it into its data folder on save). Rendering needs{' '}
                <code>uv</code> on PATH (<code>brew install uv</code>) and the F5-TTS model,
                which downloads once on the first render — afterwards everything runs offline,
                on this machine.
              </p>
            </>
          ) : tts.provider === 'piper' ? (
            <>
              <div className="field-row" style={{ marginTop: '0.5rem' }}>
                <input
                  className="search-input"
                  value={tts.piperPath}
                  placeholder="/path/to/piper"
                  aria-label="piper binary path"
                  onChange={(e) => setTts({ ...tts, piperPath: e.target.value })}
                />
                <button
                  type="button"
                  className="btn ghost tiny-btn"
                  onClick={() => void pickTtsPath('piperPath', 'piper binary')}
                >
                  Browse…
                </button>
              </div>
              <div className="field-row" style={{ marginTop: '0.5rem' }}>
                <input
                  className="search-input"
                  value={tts.voiceModelPath}
                  placeholder="/path/to/en_US-amy-medium.onnx"
                  aria-label="Piper voice model path"
                  onChange={(e) => setTts({ ...tts, voiceModelPath: e.target.value })}
                />
                <button
                  type="button"
                  className="btn ghost tiny-btn"
                  onClick={() => void pickTtsPath('voiceModelPath', 'Piper voice model')}
                >
                  Browse…
                </button>
              </div>
            </>
          ) : (
            <div className="field-row" style={{ marginTop: '0.5rem' }}>
              <input
                className="search-input"
                value={tts.macosVoice}
                placeholder="Voice name (empty = system default, e.g. Samantha)"
                aria-label="macOS voice name"
                onChange={(e) => setTts({ ...tts, macosVoice: e.target.value })}
              />
            </div>
          )}

          <div className="field-row" style={{ marginTop: '0.5rem' }}>
            <label className="model-row tiny">
              Speed
              <input
                type="range"
                min={0.5}
                max={2}
                step={0.05}
                value={tts.speed}
                onChange={(e) => setTts({ ...tts, speed: Number(e.target.value) })}
              />
              {tts.speed.toFixed(2)}×
            </label>
            <label className="model-row tiny">
              <input
                type="checkbox"
                checked={tts.mp3Enabled}
                onChange={(e) => setTts({ ...tts, mp3Enabled: e.target.checked })}
              />
              Export every render as MP3 (via ffmpeg)
            </label>
          </div>

          <div className="field-row" style={{ marginTop: '0.75rem' }}>
            <button
              type="button"
              className="btn primary tiny-btn"
              disabled={ttsBusy}
              onClick={() => void saveTts(tts)}
            >
              {ttsBusy ? 'Saving…' : 'Save voice settings'}
            </button>
          </div>
          <p className="tiny muted" style={{ marginTop: '0.5rem' }}>
            Piper voices are small .onnx files from the Piper samples page (install with{' '}
            <code>brew install piper</code>). Read-aloud works without AI; summaries and podcast
            narration use the local model. Renders stay WAV unless “Export every render as MP3”
            is on — and any past file can be converted in Audio → Convert audio to MP3.
          </p>
        </>
      )}
    </div>
  );
}
