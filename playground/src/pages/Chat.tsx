import { useState, useRef, useEffect, useCallback } from 'react';
import { chatInvoke, type Provider, type ChatMessage } from '../lib/api';

const PROVIDER_MODELS: Record<Provider, string[]> = {
  gemini: ['gemini-2.0-flash', 'gemini-2.5-pro'],
  claude: ['claude-sonnet-4-5-20250929', 'claude-haiku-4-5-20251001'],
  openai: ['gpt-4o', 'gpt-4o-mini'],
};

interface DisplayMessage {
  role: 'user' | 'ai';
  content: string;
  tokensIn?: number;
  tokensOut?: number;
}

export default function Chat() {
  const [provider, setProvider] = useState<Provider>('gemini');
  const [model, setModel] = useState(PROVIDER_MODELS.gemini[0]);
  const [temperature, setTemperature] = useState(0.7);
  const [maxTokens, setMaxTokens] = useState(1024);
  const [systemPrompt, setSystemPrompt] = useState('');
  const [messages, setMessages] = useState<DisplayMessage[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  useEffect(() => {
    setModel(PROVIDER_MODELS[provider][0]);
  }, [provider]);

  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || loading) return;

    setError(null);
    setInput('');
    const userMsg: DisplayMessage = { role: 'user', content: text };
    setMessages((prev) => [...prev, userMsg]);
    setLoading(true);

    try {
      const apiMessages: ChatMessage[] = [
        ...messages.map((m) => ({
          type: m.role === 'user' ? 'user' as const : 'ai' as const,
          content: m.content,
        })),
        { type: 'user', content: text },
      ];

      const res = await chatInvoke({
        provider,
        model,
        messages: apiMessages,
        system_prompt: systemPrompt || undefined,
        temperature,
        max_tokens: maxTokens,
      });

      setMessages((prev) => [
        ...prev,
        { role: 'ai', content: res.content, tokensIn: res.tokens_in, tokensOut: res.tokens_out },
      ]);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'An unexpected error occurred');
    } finally {
      setLoading(false);
      inputRef.current?.focus();
    }
  }, [input, loading, provider, model, temperature, maxTokens, systemPrompt, messages]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="flex h-full">
      {/* Settings Panel */}
      <aside className="w-[280px] border-r border-border bg-card p-4 overflow-y-auto shrink-0">
        <div className="space-y-5">
          <div>
            <label className="block text-xs font-medium text-muted-foreground uppercase tracking-wide mb-1.5">Provider</label>
            <select
              value={provider}
              onChange={(e) => setProvider(e.target.value as Provider)}
              className="w-full px-3 py-2 border border-border rounded-md text-sm bg-card focus:outline-none focus:ring-2 focus:ring-ring"
            >
              <option value="gemini">Gemini</option>
              <option value="claude">Claude</option>
              <option value="openai">OpenAI</option>
            </select>
          </div>

          <div>
            <label className="block text-xs font-medium text-muted-foreground uppercase tracking-wide mb-1.5">Model</label>
            <select
              value={model}
              onChange={(e) => setModel(e.target.value)}
              className="w-full px-3 py-2 border border-border rounded-md text-sm bg-card focus:outline-none focus:ring-2 focus:ring-ring"
            >
              {PROVIDER_MODELS[provider].map((m) => (
                <option key={m} value={m}>{m}</option>
              ))}
            </select>
          </div>

          <div>
            <label className="block text-xs font-medium text-muted-foreground uppercase tracking-wide mb-1.5">
              Temperature: {temperature.toFixed(1)}
            </label>
            <input
              type="range"
              min="0"
              max="2"
              step="0.1"
              value={temperature}
              onChange={(e) => setTemperature(parseFloat(e.target.value))}
              className="w-full accent-primary"
            />
            <div className="flex justify-between text-xs text-muted-foreground mt-0.5">
              <span>0.0</span>
              <span>2.0</span>
            </div>
          </div>

          <div>
            <label className="block text-xs font-medium text-muted-foreground uppercase tracking-wide mb-1.5">Max Tokens</label>
            <input
              type="number"
              min="1"
              max="128000"
              value={maxTokens}
              onChange={(e) => setMaxTokens(parseInt(e.target.value) || 1024)}
              className="w-full px-3 py-2 border border-border rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          <div>
            <label className="block text-xs font-medium text-muted-foreground uppercase tracking-wide mb-1.5">System Prompt</label>
            <textarea
              value={systemPrompt}
              onChange={(e) => setSystemPrompt(e.target.value)}
              placeholder="You are a helpful assistant..."
              rows={4}
              className="w-full px-3 py-2 border border-border rounded-md text-sm resize-none focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
        </div>
      </aside>

      {/* Chat Area */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Error Banner */}
        {error && (
          <div className="px-6 py-3 bg-destructive/10 border-b border-destructive/30">
            <p className="text-sm text-destructive">{error}</p>
          </div>
        )}

        {/* Messages */}
        <div className="flex-1 overflow-y-auto px-6 py-4">
          {messages.length === 0 && !loading && (
            <div className="flex items-center justify-center h-full text-muted-foreground text-sm">
              Send a message to start chatting
            </div>
          )}
          <div className="max-w-3xl mx-auto space-y-4">
            {messages.map((msg, i) => (
              <div key={i} className={`flex ${msg.role === 'user' ? 'justify-end' : 'justify-start'}`}>
                <div
                  className={`max-w-[80%] rounded-lg px-4 py-2.5 ${
                    msg.role === 'user'
                      ? 'bg-primary text-primary-foreground'
                      : 'bg-muted text-foreground'
                  }`}
                >
                  <p className="text-sm whitespace-pre-wrap break-words">{msg.content}</p>
                  {msg.role === 'ai' && msg.tokensIn !== undefined && (
                    <p className="text-xs text-muted-foreground mt-1.5">{msg.tokensIn} in / {msg.tokensOut} out</p>
                  )}
                </div>
              </div>
            ))}
            {loading && (
              <div className="flex justify-start">
                <div className="bg-muted rounded-lg px-4 py-2.5">
                  <div className="flex gap-1">
                    <span className="w-1.5 h-1.5 bg-muted-foreground rounded-full animate-bounce [animation-delay:0ms]" />
                    <span className="w-1.5 h-1.5 bg-muted-foreground rounded-full animate-bounce [animation-delay:150ms]" />
                    <span className="w-1.5 h-1.5 bg-muted-foreground rounded-full animate-bounce [animation-delay:300ms]" />
                  </div>
                </div>
              </div>
            )}
            <div ref={messagesEndRef} />
          </div>
        </div>

        {/* Input */}
        <div className="border-t border-border bg-card px-6 py-4 shrink-0">
          <div className="max-w-3xl mx-auto flex gap-3">
            <textarea
              ref={inputRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Send a message... (Shift+Enter for newline)"
              rows={1}
              className="flex-1 px-4 py-2.5 border border-border rounded-lg text-sm resize-none focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent"
            />
            <button
              onClick={handleSend}
              disabled={loading || !input.trim()}
              className="px-5 py-2.5 bg-primary text-primary-foreground text-sm rounded-lg hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed transition-colors shrink-0"
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
    </div>
  );
}
