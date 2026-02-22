import { useState, useRef } from 'react';
import Markdown from 'react-markdown';
import { researchInvokeStream, type ResearchSseEvent } from '../lib/api';

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
            className="w-full px-4 py-3 border border-border rounded-lg text-sm resize-none focus:outline-none focus:ring-2 focus:ring-ring"
          />
        </div>

        <div className="flex items-center gap-4">
          <div className="flex items-center gap-2">
            <label className="text-sm text-card-foreground">Agent:</label>
            <select
              value={agent}
              onChange={(e) => setAgent(e.target.value)}
              className="px-3 py-2 border border-border rounded-md text-sm bg-card focus:outline-none focus:ring-2 focus:ring-ring"
            >
              {AGENTS.map((a) => (
                <option key={a.value} value={a.value}>{a.label}</option>
              ))}
            </select>
          </div>
          <button
            onClick={() => handleStart(query)}
            disabled={loading || !query.trim()}
            className="px-6 py-2 bg-primary text-primary-foreground text-sm rounded-lg hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {loading ? 'Researching...' : '▶ Start Research'}
          </button>
        </div>

        {/* Error */}
        {error && (
          <div className="px-4 py-3 bg-destructive/10 border border-destructive/30 rounded-lg">
            <p className="text-sm text-destructive">{error}</p>
          </div>
        )}

        {/* Progress */}
        {loading && progress && (
          <div className="space-y-2">
            <div className="w-full bg-muted rounded-full h-2">
              <div className="bg-primary h-2 rounded-full animate-pulse" style={{ width: '75%' }} />
            </div>
            <p className="text-sm text-muted-foreground">{progress}</p>
          </div>
        )}

        {/* Result */}
        {result && (
          <div className="border border-border rounded-lg bg-card">
            <div className="flex justify-end px-4 pt-3">
              <button
                onClick={handleCopy}
                className="text-xs text-muted-foreground hover:text-foreground border border-border rounded px-2 py-1"
              >
                {copied ? 'Copied!' : 'Copy'}
              </button>
            </div>
            <div className="px-6 pb-6 prose prose-sm max-w-none text-foreground">
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
              className="flex-1 px-4 py-2.5 border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
            <button
              onClick={handleFollowUp}
              disabled={loading || !followUp.trim()}
              className="px-5 py-2.5 bg-primary text-primary-foreground text-sm rounded-lg hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed"
            >
              Send
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
