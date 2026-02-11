import { useState, useRef, useEffect, useCallback } from 'react';
import {
  agentInvokeStream,
  getApiKey,
  type Provider,
  type ChatMessage,
  type AgentSseEvent,
} from '../lib/api';

const PROVIDER_MODELS: Record<Provider, string[]> = {
  gemini: ['gemini-2.0-flash', 'gemini-2.5-pro'],
  claude: ['claude-sonnet-4-5-20250929', 'claude-haiku-4-5-20251001'],
  openai: ['gpt-4o', 'gpt-4o-mini'],
};

const BUILTIN_TOOLS = [
  { id: 'calculator', label: 'Calculator', desc: 'Arithmetic' },
  { id: 'datetime', label: 'DateTime', desc: 'Current time' },
  { id: 'web_search', label: 'Web Search', desc: 'Search the web' },
];

interface DisplayMessage {
  kind: 'user' | 'ai' | 'tool_call' | 'tool_result';
  content: string;
  toolName?: string;
  args?: Record<string, unknown>;
}

interface TraceStep {
  stepNumber: number;
  nodeName: string;
  summary: string;
  details: AgentSseEvent[];
  status: 'active' | 'done';
}

export default function Agent() {
  const [provider, setProvider] = useState<Provider>('gemini');
  const [model, setModel] = useState(PROVIDER_MODELS.gemini[0]);
  const [enabledTools, setEnabledTools] = useState<Set<string>>(new Set(['calculator', 'datetime']));
  const [recursionLimit, setRecursionLimit] = useState(25);
  const [messages, setMessages] = useState<DisplayMessage[]>([]);
  const [traceSteps, setTraceSteps] = useState<TraceStep[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [totalSteps, setTotalSteps] = useState<number | null>(null);

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  useEffect(() => {
    setModel(PROVIDER_MODELS[provider][0]);
  }, [provider]);

  const toggleTool = (id: string) => {
    setEnabledTools((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  // Track conversation history for API
  const conversationRef = useRef<ChatMessage[]>([]);

  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || loading) return;

    if (!getApiKey(provider)) {
      setError(`API key for ${provider} is not set. Click "API Keys" in the header to configure.`);
      return;
    }

    setError(null);
    setInput('');
    setTotalSteps(null);
    setMessages((prev) => [...prev, { kind: 'user', content: text }]);
    setTraceSteps([]);
    setLoading(true);

    conversationRef.current.push({ type: 'user', content: text });

    const controller = new AbortController();
    abortRef.current = controller;

    try {
      await agentInvokeStream(
        {
          provider,
          model,
          tools: [...enabledTools],
          messages: [...conversationRef.current],
          recursion_limit: recursionLimit,
        },
        (event: AgentSseEvent) => {
          switch (event.type) {
            case 'step':
              setTraceSteps((prev) => {
                const updated = prev.map((s) => ({ ...s, status: 'done' as const }));
                return [
                  ...updated,
                  {
                    stepNumber: event.step_number!,
                    nodeName: event.node_name!,
                    summary: event.summary!,
                    details: [event],
                    status: 'active' as const,
                  },
                ];
              });
              break;
            case 'tool_call':
              setMessages((prev) => [
                ...prev,
                {
                  kind: 'tool_call',
                  content: `${event.tool_name}(${JSON.stringify(event.arguments)})`,
                  toolName: event.tool_name,
                  args: event.arguments,
                },
              ]);
              setTraceSteps((prev) => {
                if (prev.length === 0) return prev;
                const last = { ...prev[prev.length - 1] };
                last.details = [...last.details, event];
                return [...prev.slice(0, -1), last];
              });
              break;
            case 'tool_result':
              setMessages((prev) => [
                ...prev,
                { kind: 'tool_result', content: event.result!, toolName: event.tool_name },
              ]);
              setTraceSteps((prev) => {
                if (prev.length === 0) return prev;
                const last = { ...prev[prev.length - 1] };
                last.details = [...last.details, event];
                return [...prev.slice(0, -1), last];
              });
              break;
            case 'message':
              setMessages((prev) => [...prev, { kind: 'ai', content: event.content! }]);
              conversationRef.current.push({ type: 'ai', content: event.content! });
              break;
            case 'done':
              setTotalSteps(event.total_steps!);
              setTraceSteps((prev) => prev.map((s) => ({ ...s, status: 'done' as const })));
              break;
            case 'error':
              setError(event.message!);
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
      abortRef.current = null;
      inputRef.current?.focus();
    }
  }, [input, loading, provider, model, enabledTools, recursionLimit]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="flex h-full">
      {/* Tool Settings Panel */}
      <aside className="w-[260px] border-r border-gray-200 bg-white p-4 overflow-y-auto shrink-0">
        <div className="space-y-5">
          <div>
            <label className="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Tools</label>
            <div className="space-y-2">
              {BUILTIN_TOOLS.map((tool) => (
                <label key={tool.id} className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={enabledTools.has(tool.id)}
                    onChange={() => toggleTool(tool.id)}
                    className="rounded border-gray-300 accent-gray-900"
                  />
                  <span className="text-sm text-gray-800">{tool.label}</span>
                  <span className="text-xs text-gray-400">— {tool.desc}</span>
                </label>
              ))}
            </div>
          </div>

          <hr className="border-gray-200" />

          <div>
            <label className="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-1.5">Provider</label>
            <select
              value={provider}
              onChange={(e) => setProvider(e.target.value as Provider)}
              className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm bg-white focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              <option value="gemini">Gemini</option>
              <option value="claude">Claude</option>
              <option value="openai">OpenAI</option>
            </select>
          </div>

          <div>
            <label className="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-1.5">Model</label>
            <select
              value={model}
              onChange={(e) => setModel(e.target.value)}
              className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm bg-white focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              {PROVIDER_MODELS[provider].map((m) => (
                <option key={m} value={m}>{m}</option>
              ))}
            </select>
          </div>

          <div>
            <label className="block text-xs font-medium text-gray-500 uppercase tracking-wide mb-1.5">
              Recursion Limit: {recursionLimit}
            </label>
            <input
              type="range"
              min="1"
              max="100"
              value={recursionLimit}
              onChange={(e) => setRecursionLimit(parseInt(e.target.value))}
              className="w-full accent-gray-900"
            />
            <div className="flex justify-between text-xs text-gray-400 mt-0.5">
              <span>1</span>
              <span>100</span>
            </div>
          </div>
        </div>
      </aside>

      {/* Chat Area */}
      <div className="flex-1 flex flex-col min-w-0">
        {error && (
          <div className="px-6 py-3 bg-red-50 border-b border-red-200">
            <p className="text-sm text-red-700">{error}</p>
          </div>
        )}

        <div className="flex-1 overflow-y-auto px-6 py-4">
          {messages.length === 0 && !loading && (
            <div className="flex items-center justify-center h-full text-gray-400 text-sm">
              Send a message to start an agent conversation
            </div>
          )}
          <div className="max-w-2xl mx-auto space-y-3">
            {messages.map((msg, i) => {
              if (msg.kind === 'user') {
                return (
                  <div key={i} className="flex justify-end">
                    <div className="max-w-[80%] bg-gray-900 text-white rounded-lg px-4 py-2.5">
                      <p className="text-sm whitespace-pre-wrap break-words">{msg.content}</p>
                    </div>
                  </div>
                );
              }
              if (msg.kind === 'ai') {
                return (
                  <div key={i} className="flex justify-start">
                    <div className="max-w-[80%] bg-white border border-gray-200 rounded-lg px-4 py-2.5">
                      <p className="text-sm whitespace-pre-wrap break-words text-gray-800">{msg.content}</p>
                    </div>
                  </div>
                );
              }
              if (msg.kind === 'tool_call') {
                return (
                  <div key={i} className="flex justify-start">
                    <div className="bg-blue-50 border border-blue-200 rounded-lg px-3 py-2 text-xs font-mono text-blue-800">
                      <span className="mr-1">🔧</span>
                      {msg.content}
                    </div>
                  </div>
                );
              }
              // tool_result
              return (
                <div key={i} className="flex justify-start pl-4">
                  <div className="bg-gray-50 border border-gray-200 rounded px-3 py-1.5 text-xs font-mono text-gray-600">
                    → {msg.content}
                  </div>
                </div>
              );
            })}
            {loading && (
              <div className="flex justify-start">
                <div className="bg-white border border-gray-200 rounded-lg px-4 py-2.5">
                  <div className="flex gap-1">
                    <span className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce [animation-delay:0ms]" />
                    <span className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce [animation-delay:150ms]" />
                    <span className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce [animation-delay:300ms]" />
                  </div>
                </div>
              </div>
            )}
            <div ref={messagesEndRef} />
          </div>
        </div>

        <div className="border-t border-gray-200 bg-white px-6 py-4 shrink-0">
          <div className="max-w-2xl mx-auto flex gap-3">
            <textarea
              ref={inputRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Send a message..."
              rows={1}
              className="flex-1 px-4 py-2.5 border border-gray-300 rounded-lg text-sm resize-none focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <button
              onClick={handleSend}
              disabled={loading || !input.trim()}
              className="px-5 py-2.5 bg-gray-900 text-white text-sm rounded-lg hover:bg-gray-800 disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
            >
              {loading ? (
                <svg className="animate-spin h-4 w-4" viewBox="0 0 24 24" fill="none">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                </svg>
              ) : 'Send'}
            </button>
          </div>
        </div>
      </div>

      {/* Trace Panel */}
      <aside className="w-[340px] border-l border-gray-200 bg-white p-4 overflow-y-auto shrink-0">
        <h3 className="text-xs font-medium text-gray-500 uppercase tracking-wide mb-3">Execution Trace</h3>
        {traceSteps.length === 0 && !loading && (
          <p className="text-sm text-gray-400">No trace yet</p>
        )}
        <div className="space-y-3">
          {traceSteps.map((step, i) => (
            <TraceCard key={i} step={step} />
          ))}
        </div>
        {totalSteps !== null && (
          <div className="mt-4 px-3 py-2 bg-green-50 border border-green-200 rounded-md text-sm text-green-700">
            ✅ {totalSteps} step{totalSteps !== 1 ? 's' : ''} completed
          </div>
        )}
      </aside>
    </div>
  );
}

function TraceCard({ step }: { step: TraceStep }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div
      className={`border rounded-lg p-3 cursor-pointer transition-colors ${
        step.status === 'active' ? 'border-blue-300 bg-blue-50' : 'border-gray-200'
      }`}
      onClick={() => setExpanded(!expanded)}
    >
      <div className="flex items-center gap-2">
        {step.status === 'active' ? (
          <svg className="animate-spin h-3.5 w-3.5 text-blue-500" viewBox="0 0 24 24" fill="none">
            <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
            <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
          </svg>
        ) : (
          <span className="text-xs text-gray-400">Step {step.stepNumber}</span>
        )}
        <span className={`text-xs px-1.5 py-0.5 rounded font-medium ${
          step.nodeName === 'agent' ? 'bg-purple-100 text-purple-700' : 'bg-orange-100 text-orange-700'
        }`}>
          {step.nodeName}
        </span>
        <span className="text-xs text-gray-600 truncate flex-1">{step.summary}</span>
      </div>
      {expanded && step.details.length > 0 && (
        <pre className="mt-2 text-xs bg-gray-50 rounded p-2 overflow-x-auto text-gray-600 max-h-48 overflow-y-auto">
          {JSON.stringify(step.details, null, 2)}
        </pre>
      )}
    </div>
  );
}
