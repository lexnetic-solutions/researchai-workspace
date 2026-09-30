import { useEffect, useState } from 'react';
import { backend } from '../backend/client';
import { useStore } from '../state/store';

export function StatusBar() {
  const { native, activeProject, settings } = useStore();
  const [engineOk, setEngineOk] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    void backend
      .probeDocumentEngine()
      .then((ok) => {
        if (!cancelled) setEngineOk(ok);
      })
      .catch(() => {
        if (!cancelled) setEngineOk(false);
      });
    return () => {
      cancelled = true;
    };
  }, [native]);

  return (
    <footer className="statusbar">
      <span className="status-item">
        <span className={`status-dot ${engineOk ? 'ok' : engineOk === null ? 'unknown' : 'down'}`} />
        Document engine {engineOk === null ? 'checking…' : engineOk ? 'online' : 'offline'}
      </span>
      <span className="status-item">{native ? 'Native shell' : 'Browser preview'}</span>
      <span className="status-item">
        {activeProject ? `Project: ${activeProject.name}` : 'No project selected'}
      </span>
      <span className="status-spacer" />
      <span className="status-item muted">
        {settings ? `${settings.maxConcurrentJobs} concurrent job(s) · OCR ${settings.ocrEnabled ? 'on' : 'off'}` : ''}
      </span>
    </footer>
  );
}

