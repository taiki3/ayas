import { useState } from 'react';
import { NavLink, Outlet } from 'react-router-dom';
import ApiKeysModal from './ApiKeysModal';

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

export default function Layout() {
  const [keysOpen, setKeysOpen] = useState(false);

  return (
    <div className="flex flex-col h-screen min-w-[1024px] bg-gray-50">
      <header className="flex items-center justify-between px-6 h-14 border-b border-gray-200 bg-white shrink-0">
        <div className="flex items-center gap-8">
          <h1 className="text-base font-semibold text-gray-900 tracking-tight">Ayas Playground</h1>
          <nav className="flex gap-1">
            {TABS.map(({ to, label }) => (
              <NavLink
                key={to}
                to={to}
                end={to === '/'}
                className={({ isActive }) =>
                  `px-3 py-1.5 text-sm rounded-md transition-colors ${
                    isActive
                      ? 'bg-gray-900 text-white'
                      : 'text-gray-600 hover:text-gray-900 hover:bg-gray-100'
                  }`
                }
              >
                {label}
              </NavLink>
            ))}
          </nav>
        </div>
        <button
          onClick={() => setKeysOpen(true)}
          className="px-3 py-1.5 text-sm text-gray-600 hover:text-gray-900 border border-gray-200 rounded-md hover:bg-gray-50"
        >
          API Keys
        </button>
      </header>

      <main className="flex-1 overflow-hidden">
        <Outlet />
      </main>

      <ApiKeysModal open={keysOpen} onClose={() => setKeysOpen(false)} />
    </div>
  );
}
