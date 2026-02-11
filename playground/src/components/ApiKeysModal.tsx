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
        className="bg-white rounded-lg shadow-xl w-full max-w-md p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-lg font-semibold text-gray-900 mb-4">API Keys</h2>
        <div className="space-y-4">
          {PROVIDERS.map(({ key, label }) => (
            <div key={key}>
              <label className="block text-sm font-medium text-gray-700 mb-1">{label}</label>
              <input
                type="password"
                value={keys[key] || ''}
                onChange={(e) => setKeys((prev) => ({ ...prev, [key]: e.target.value }))}
                placeholder={`Enter ${label}`}
                className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
              />
            </div>
          ))}
        </div>
        <div className="flex justify-end gap-2 mt-6">
          <button
            onClick={onClose}
            className="px-4 py-2 text-sm text-gray-600 hover:text-gray-800"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            className="px-4 py-2 text-sm bg-gray-900 text-white rounded-md hover:bg-gray-800"
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
