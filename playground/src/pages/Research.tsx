import { useState, useRef } from 'react';
import Markdown from 'react-markdown';
import { researchInvokeStream, getApiKey, type ResearchSseEvent } from '../lib/api';

const AGENTS = [
  { value: '', label: 'Default' },
  { value: 'deep-research-pro', label: 'Deep Research Pro' },
  { value: 'gemini', label: 'Gemini' },
];

export default function Research() {
  const [query, setQuery] = useState('');
  const [agent, setAgent] = useState('');
  const [loading, setLoading] = useState(false);
  const [progress, setProgress] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [interactionId, setInteractionId] = useState<string | null>(null);
  const [followUp, setFollowUp] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const handleStart = async (queryText: string, prevId?: string) => {
    if (!queryText.trim() || loading) return;

    if (!getApiKey('gemini')) {
      setError('Gemini API key is required for Research. Click "API Keys" in the header to configure.');
      return;
    }

    setError(null);
    setLoading(true);
    setProgress('Starting research...');
    setResult(null);
    setCopied(false);

    const controller = new AbortController();
    abortRef.current = controller;

    try {
      await researchInvokeStream(
        queryText,
        agent || undefined,
        prevId,
        (event: ResearchSseEvent) => {
          switch (event.type) {
            case 'progress':
              setProgress(event.message || 'Researching...');
              break;
            case 'complete':
              setResult(event.text || '');
              setInteractionId(event.interaction_id || null);
              setProgress(null);
              break;
            case 'error':
              setError(event.message || 'Research failed');
              setProgress(null);
              break;
          }
        },
        controller.signal,
      );
    } catch (err) {
      if (!(err instanceof DOMException && err.name === 'AbortError')) {
        setError(err instanceof Error ? err.message : 'An unexpected error occurred');
      }
    } finally {
      setLoading(false);
      setProgress(null);
      abortRef.current = null;
    }
  };

  const handleFollowUp = () => {
    if (!followUp.trim()) return;
    const q = followUp.trim();
    setFollowUp('');
    handleStart(q, interactionId || undefined);
  };

  const handleCopy = () => {
    if (result) {
      navigator.clipboard.writeText(result);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="max-w-3xl mx-auto px-6 py-8 space-y-6">
        {/* Query Input */}
        <div>
          <textarea
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Enter your research query..."
            rows={4}
            className="w-full px-4 py-3 border border-gray-300 rounded-lg text-sm resize-none focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>

        <div className="flex items-center gap-4">
          <div className="flex items-center gap-2">
            <label className="text-sm text-gray-600">Agent:</label>
            <select
              value={agent}
              onChange={(e) => setAgent(e.target.value)}
              className="px-3 py-2 border border-gray-300 rounded-md text-sm bg-white focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              {AGENTS.map((a) => (
                <option key={a.value} value={a.value}>{a.label}</option>
              ))}
            </select>
          </div>
          <button
            onClick={() => handleStart(query)}
            disabled={loading || !query.trim()}
            className="px-6 py-2 bg-gray-900 text-white text-sm rounded-lg hover:bg-gray-800 disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {loading ? 'Researching...' : '▶ Start Research'}
          </button>
        </div>

        {/* Error */}
        {error && (
          <div className="px-4 py-3 bg-red-50 border border-red-200 rounded-lg">
            <p className="text-sm text-red-700">{error}</p>
          </div>
        )}

        {/* Progress */}
        {loading && progress && (
          <div className="space-y-2">
            <div className="w-full bg-gray-200 rounded-full h-2">
              <div className="bg-blue-600 h-2 rounded-full animate-pulse" style={{ width: '75%' }} />
            </div>
            <p className="text-sm text-gray-500">{progress}</p>
          </div>
        )}

        {/* Result */}
        {result && (
          <div className="border border-gray-200 rounded-lg bg-white">
            <div className="flex justify-end px-4 pt-3">
              <button
                onClick={handleCopy}
                className="text-xs text-gray-500 hover:text-gray-700 border border-gray-200 rounded px-2 py-1"
              >
                {copied ? 'Copied!' : 'Copy'}
              </button>
            </div>
            <div className="px-6 pb-6 prose prose-sm max-w-none text-gray-800">
              <Markdown>{result}</Markdown>
            </div>
          </div>
        )}

        {/* Follow-up */}
        {result && (
          <div className="flex gap-3">
            <input
              type="text"
              value={followUp}
              onChange={(e) => setFollowUp(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') handleFollowUp(); }}
              placeholder="Ask a follow-up question..."
              className="flex-1 px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <button
              onClick={handleFollowUp}
              disabled={loading || !followUp.trim()}
              className="px-5 py-2.5 bg-gray-900 text-white text-sm rounded-lg hover:bg-gray-800 disabled:opacity-40 disabled:cursor-not-allowed"
            >
              Send
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
