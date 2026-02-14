import { useState, useEffect } from 'react';

interface ApiKeysModalProps {
  open: boolean;
  onClose: () => void;
}

const PROVIDERS = [
  { key: 'gemini', label: 'Gemini API Key' },
  { key: 'claude', label: 'Anthropic API Key' },
  { key: 'openai', label: 'OpenAI API Key' },
] as const;

export default function ApiKeysModal({ open, onClose }: ApiKeysModalProps) {
  const [keys, setKeys] = useState<Record<string, string>>({});

  useEffect(() => {
    if (open) {
      try {
        const saved = JSON.parse(localStorage.getItem('ayas-api-keys') || '{}');
        setKeys(saved);
      } catch {
        setKeys({});
      }
    }
  }, [open]);

  if (!open) return null;

  const handleSave = () => {
    localStorage.setItem('ayas-api-keys', JSON.stringify(keys));
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={onClose}>
      <div
        className="bg-card rounded-lg shadow-xl w-full max-w-md p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-lg font-semibold text-foreground mb-4">API Keys</h2>
        <div className="space-y-4">
          {PROVIDERS.map(({ key, label }) => (
            <div key={key}>
              <label className="block text-sm font-medium text-card-foreground mb-1">{label}</label>
              <input
                type="password"
                value={keys[key] || ''}
                onChange={(e) => setKeys((prev) => ({ ...prev, [key]: e.target.value }))}
                placeholder={`Enter ${label}`}
                className="w-full px-3 py-2 border border-border rounded-md text-sm bg-card focus:outline-none focus:ring-2 focus:ring-ring"
              />
            </div>
          ))}
        </div>
        <div className="flex justify-end gap-2 mt-6">
          <button
            onClick={onClose}
            className="px-4 py-2 text-sm text-muted-foreground hover:text-foreground transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            className="px-4 py-2 text-sm bg-primary text-primary-foreground rounded-md hover:opacity-90 transition-opacity"
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
