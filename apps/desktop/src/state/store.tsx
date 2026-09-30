import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import type { AppSettings, Project } from '@researchai/shared-types';
import { backend, isNative } from '../backend/client';

/** Sidebar navigation ids (spec §25 navigation, Phase 0 subset). */
export type ViewId =
  | 'home'
  | 'library'
  | 'projects'
  | 'documents'
  | 'search'
  | 'research'
  | 'notes'
  | 'evidence'
  | 'audio'
  | 'exports'
  | 'settings';

export interface Toast {
  readonly id: number;
  readonly kind: 'success' | 'error' | 'info';
  readonly message: string;
}

interface AppState {
  view: ViewId;
  setView: (view: ViewId) => void;
  projects: Project[];
  activeProjectId: string | null;
  setActiveProjectId: (id: string | null) => void;
  activeProject: Project | null;
  settings: AppSettings | null;
  setSettings: (s: AppSettings) => void;
  toasts: Toast[];
  pushToast: (kind: Toast['kind'], message: string) => void;
  dismissToast: (id: number) => void;
  reloadProjects: () => Promise<void>;
  loading: boolean;
  backendError: string | null;
  native: boolean;
}

const StoreContext = createContext<AppState | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [view, setView] = useState<ViewId>('home');
  const [projects, setProjects] = useState<Project[]>([]);
  const [activeProjectId, setActiveProjectId] = useState<string | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [loading, setLoading] = useState(true);
  const [backendError, setBackendError] = useState<string | null>(null);

  const pushToast = useCallback((kind: Toast['kind'], message: string) => {
    const id = Date.now() + Math.floor(Math.random() * 1000);
    setToasts((prev) => [...prev, { id, kind, message }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 5000);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const reloadProjects = useCallback(async () => {
    try {
      const list = await backend.listProjects();
      setProjects(list);
      setBackendError(null);
    } catch (err) {
      setBackendError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const [s] = await Promise.all([backend.getSettings(), reloadProjects()]);
        setSettings(s);
        applyTheme(s.theme);
      } catch (err) {
        setBackendError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    })();
  }, [reloadProjects]);

  const activeProject = useMemo(
    () => projects.find((p) => p.id === activeProjectId) ?? null,
    [projects, activeProjectId],
  );

  const value: AppState = {
    view,
    setView,
    projects,
    activeProjectId,
    setActiveProjectId,
    activeProject,
    settings,
    setSettings,
    toasts,
    pushToast,
    dismissToast,
    reloadProjects,
    loading,
    backendError,
    native: isNative,
  };

  return <StoreContext.Provider value={value}>{children}</StoreContext.Provider>;
}

export function useStore(): AppState {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error('useStore must be used within <StoreProvider>');
  return ctx;
}

export function applyTheme(theme: AppSettings['theme']): void {
  const root = document.documentElement;
  root.classList.remove('theme-light', 'theme-dark');
  if (theme === 'light' || theme === 'dark') {
    root.classList.add(`theme-${theme}`);
  } else if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
    root.classList.add('theme-dark');
  } else {
    root.classList.add('theme-light');
  }
}
