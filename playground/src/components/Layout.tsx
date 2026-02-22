import { useState, useEffect } from 'react';
import { NavLink, Outlet } from 'react-router-dom';
import { fetchEnvKeys, type EnvKeys } from '../lib/api';

const API_URL = import.meta.env.VITE_API_URL || '';

const TABS = [
  { to: '/', label: 'Chat' },
  { to: '/agent', label: 'Agent' },
  { to: '/graph', label: 'Graph' },
  { to: '/research', label: 'Research' },
  { to: '/pipeline', label: 'Pipeline' },
  { to: '/traces', label: 'Traces' },
  { to: '/time-travel', label: 'TimeTravel' },
  { to: '/projects', label: 'Projects' },
  { to: '/dashboard', label: 'Dashboard' },
];

type BackendStatus = 'checking' | 'connected' | 'disconnected';

export default function Layout() {
  const [backendStatus, setBackendStatus] = useState<BackendStatus>('checking');
  const [envKeys, setEnvKeys] = useState<EnvKeys | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function checkHealth() {
      setBackendStatus('checking');
      try {
        const resp = await fetch(`${API_URL}/health`, { signal: AbortSignal.timeout(3000) });
        if (!cancelled) setBackendStatus(resp.ok ? 'connected' : 'disconnected');
      } catch {
        if (!cancelled) setBackendStatus('disconnected');
      }
    }
    checkHealth();
    const interval = setInterval(checkHealth, 30000);
    return () => { cancelled = true; clearInterval(interval); };
  }, []);

  useEffect(() => {
    fetchEnvKeys().then(setEnvKeys);
  }, []);

  return (
    <div className="flex flex-col h-screen min-w-[1024px] bg-surface">
      <header className="flex items-center justify-between px-6 h-16 border-b border-border bg-card shrink-0">
        <div className="flex items-center gap-6">
          <div className="flex items-center gap-2.5">
            <span
              className="font-serif text-3xl font-bold tracking-tight leading-tight"
              style={{
                backgroundImage: 'linear-gradient(135deg, #BBBF45 0%, #BFA454 60%, #D93240 100%)',
                WebkitBackgroundClip: 'text',
                WebkitTextFillColor: 'transparent',
                backgroundClip: 'text',
              }}
            >
              Ayas Playground
            </span>
            <span
              className={`h-2 w-2 rounded-full shrink-0 ${
                backendStatus === 'connected' ? 'bg-success' :
                backendStatus === 'disconnected' ? 'bg-destructive' :
                'bg-warning animate-pulse'
              }`}
              title={`Backend: ${backendStatus}`}
            />
          </div>

          <nav className="flex items-center gap-1">
            {TABS.map(({ to, label }) => (
              <NavLink
                key={to}
                to={to}
                end={to === '/'}
                className={({ isActive }) =>
                  `px-3 py-1.5 text-sm font-medium rounded-md transition-colors ${
                    isActive
                      ? 'bg-primary text-primary-foreground'
                      : 'text-muted-foreground hover:text-foreground hover:bg-muted'
                  }`
                }
              >
                {label}
              </NavLink>
            ))}
          </nav>
        </div>
        <button
          disabled
          className="px-3 py-1.5 text-sm border rounded-md text-muted-foreground/50 border-border/50 cursor-not-allowed"
          title={envKeys ? `Keys: ${[envKeys.gemini && 'Gemini', envKeys.claude && 'Claude', envKeys.openai && 'OpenAI'].filter(Boolean).join(', ')}` : 'Checking...'}
        >
          API Keys (env)
        </button>
      </header>

      <main className="flex-1 overflow-hidden">
        <Outlet />
      </main>

    </div>
  );
}
