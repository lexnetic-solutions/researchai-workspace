import { useCallback, useEffect, useState } from 'react';
import type {
  AiSettings,
  DiagnosticsReport,
  LocalModel,
  ModelDownloadEvent,
  RuntimeStatus,
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
    </section>
  );
}
