import { useState, useRef } from 'react';
import Markdown from 'react-markdown';
import { pipelineInvokeStream, getApiKey, type PipelineSseEvent, type PipelineInvokeParams } from '../lib/api';

interface HypothesisCardFull {
  index: number;
  title: string;
  score: number;
  physical_contradiction: string;
  cap_id_fingerprint: string;
  verdict_tag: string;
  verdict_reason: string;
}

interface Step3Report {
  title: string;
  text: string | null;
  error: string | null;
  loading: boolean;
}

function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const handleCopy = () => {
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };
  return (
    <button
      onClick={handleCopy}
      className="text-xs text-gray-500 hover:text-gray-700 border border-gray-200 rounded px-2 py-1"
    >
      {copied ? 'Copied!' : label}
    </button>
  );
}

function DownloadButton({ filename, content, label = 'Download' }: { filename: string; content: string; label?: string }) {
  const handleDownload = () => {
    const blob = new Blob([content], { type: 'text/markdown' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = filename;
    a.click();
    URL.revokeObjectURL(a.href);
  };
  return (
    <button
      onClick={handleDownload}
      className="text-xs text-gray-500 hover:text-gray-700 border border-gray-200 rounded px-2 py-1"
    >
      {label}
    </button>
  );
}

export default function Pipeline() {
  const [mode, setMode] = useState<'llm' | 'manual'>('llm');
  const [hypothesisCount, setHypothesisCount] = useState(3);
  const [needsText, setNeedsText] = useState('');
  const [seedsText, setSeedsText] = useState('');
  const [manualHypotheses, setManualHypotheses] = useState<string[]>(['']);
  const [loading, setLoading] = useState(false);
  const [currentStep, setCurrentStep] = useState<number | null>(null);
  const [stepDescription, setStepDescription] = useState<string | null>(null);
  const [completedSteps, setCompletedSteps] = useState<Map<number, string>>(new Map());
  const [hypotheses, setHypotheses] = useState<HypothesisCardFull[]>([]);
  const [hypothesesJson, setHypothesesJson] = useState<Record<string, unknown> | null>(null);
  const [jsonViewOpen, setJsonViewOpen] = useState(false);
  const [step1Text, setStep1Text] = useState<string | null>(null);
  const [step1Open, setStep1Open] = useState(false);
  const [step3Results, setStep3Results] = useState<Step3Report[]>([]);
  const [step3Open, setStep3Open] = useState<Set<number>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [hypothesisDetailOpen, setHypothesisDetailOpen] = useState<Set<number>>(new Set());
  const abortRef = useRef<AbortController | null>(null);

  const handleStart = async () => {
    if (loading) return;

    if (!getApiKey('gemini')) {
      setError('Gemini API key is required. Click "API Keys" in the header to configure.');
      return;
    }

    if (mode === 'manual') {
      const validTitles = manualHypotheses.filter(t => t.trim());
      if (validTitles.length === 0) {
        setError('Manual mode requires at least one hypothesis title.');
        return;
      }
    }

    setError(null);
    setLoading(true);
    setCurrentStep(null);
    setStepDescription(null);
    setCompletedSteps(new Map());
    setHypotheses([]);
    setHypothesesJson(null);
    setJsonViewOpen(false);
    setStep1Text(null);
    setStep1Open(false);
    setStep3Results([]);
    setStep3Open(new Set());
    setDone(false);
    setHypothesisDetailOpen(new Set());

    const controller = new AbortController();
    abortRef.current = controller;

    const params: PipelineInvokeParams = {
      mode,
      hypothesis_count: hypothesisCount,
      needs: needsText.trim() || undefined,
      seeds: seedsText.trim() || undefined,
      hypotheses: mode === 'manual'
        ? manualHypotheses.filter(t => t.trim()).map(t => ({ title: t.trim() }))
        : undefined,
    };

    try {
      await pipelineInvokeStream(
        params,
        (event: PipelineSseEvent) => {
          switch (event.type) {
            case 'step_start':
              setCurrentStep(event.step ?? null);
              setStepDescription(event.description ?? null);
              break;
            case 'step_complete':
              if (event.step != null && event.summary) {
                setCompletedSteps((prev) => {
                  const next = new Map(prev);
                  next.set(event.step!, event.summary!);
                  return next;
                });
              }
              break;
            case 'hypothesis':
              if (event.title != null) {
                setHypotheses((prev) => [
                  ...prev,
                  {
                    index: event.index ?? prev.length,
                    title: event.title!,
                    score: event.score ?? 0,
                    physical_contradiction: event.physical_contradiction ?? '',
                    cap_id_fingerprint: event.cap_id_fingerprint ?? '',
                    verdict_tag: event.verdict_tag ?? '',
                    verdict_reason: event.verdict_reason ?? '',
                  },
                ]);
              }
              break;
            case 'step3_start':
              if (event.index != null && event.title) {
                setStep3Results((prev) => {
                  const next = [...prev];
                  next[event.index!] = { title: event.title!, text: null, error: null, loading: true };
                  return next;
                });
              }
              break;
            case 'step3_complete':
              if (event.index != null) {
                setStep3Results((prev) => {
                  const next = [...prev];
                  next[event.index!] = { title: event.title ?? '', text: event.text ?? '', error: null, loading: false };
                  return next;
                });
              }
              break;
            case 'step3_error':
              if (event.index != null) {
                setStep3Results((prev) => {
                  const next = [...prev];
                  next[event.index!] = { title: event.title ?? '', text: null, error: event.message ?? 'Unknown error', loading: false };
                  return next;
                });
              }
              break;
            case 'complete':
              setStep1Text(event.step1_text ?? null);
              setHypothesesJson((event.hypotheses as Record<string, unknown>) ?? null);
              setDone(true);
              setCurrentStep(null);
              setStepDescription(null);
              break;
            case 'error':
              setError(event.message ?? 'Pipeline failed');
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
    }
  };

  const handleCancel = () => {
    abortRef.current?.abort();
    setLoading(false);
  };

  const toggleStep3 = (index: number) => {
    setStep3Open((prev) => {
      const next = new Set(prev);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  };

  const toggleHypothesisDetail = (index: number) => {
    setHypothesisDetailOpen((prev) => {
      const next = new Set(prev);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  };

  const buildAllResultsMarkdown = () => {
    const parts: string[] = [];
    if (step1Text) {
      parts.push('# STEP 1: 仮説生成レポート\n\n' + step1Text);
    }
    if (hypotheses.length > 0) {
      parts.push('# STEP 2: 抽出された仮説\n\n' + hypotheses.map((h, i) =>
        `## ${i + 1}. ${h.title}\n- Score: ${h.score.toFixed(2)}\n- Cap-ID: ${h.cap_id_fingerprint}\n- Verdict: ${h.verdict_tag}\n- Physical Contradiction: ${h.physical_contradiction}\n- Verdict Reason: ${h.verdict_reason}`
      ).join('\n\n'));
    }
    if (step3Results.length > 0) {
      parts.push('# STEP 3: 深掘りレポート\n\n' + step3Results.map((r, i) => {
        if (r.text) return `## ${i + 1}. ${r.title}\n\n${r.text}`;
        if (r.error) return `## ${i + 1}. ${r.title}\n\nError: ${r.error}`;
        return '';
      }).filter(Boolean).join('\n\n---\n\n'));
    }
    return parts.join('\n\n---\n\n');
  };

  const updateManualHypothesis = (index: number, value: string) => {
    setManualHypotheses(prev => {
      const next = [...prev];
      next[index] = value;
      return next;
    });
  };

  const removeManualHypothesis = (index: number) => {
    setManualHypotheses(prev => prev.filter((_, i) => i !== index));
  };

  const addManualHypothesis = () => {
    setManualHypotheses(prev => [...prev, '']);
  };

  const isManual = mode === 'manual';

  return (
    <div className="h-full flex">
      {/* Left Panel */}
      <div className="w-[280px] shrink-0 border-r border-gray-200 bg-gray-50 overflow-y-auto p-4 space-y-4">
        <h2 className="text-sm font-semibold text-gray-900">Hypothesis Pipeline</h2>

        {/* Mode select */}
        <div>
          <label className="block text-xs font-medium text-gray-600 mb-1">MODE</label>
          <select
            value={mode}
            onChange={(e) => setMode(e.target.value as 'llm' | 'manual')}
            disabled={loading}
            className="w-full text-sm border border-gray-300 rounded-md px-2 py-1.5 bg-white"
          >
            <option value="llm">LLM (3-step auto)</option>
            <option value="manual">Manual (STEP 3 only)</option>
          </select>
        </div>

        {/* NEEDS textarea */}
        <div>
          <label className="block text-xs font-medium text-gray-600 mb-1">
            NEEDS <span className="text-gray-400">(empty = demo)</span>
          </label>
          <textarea
            value={needsText}
            onChange={(e) => setNeedsText(e.target.value)}
            disabled={loading}
            rows={6}
            placeholder="target_specification.txt の内容..."
            className="w-full text-xs border border-gray-300 rounded-md px-2 py-1.5 resize-y font-mono"
          />
        </div>

        {/* SEEDS textarea */}
        <div>
          <label className="block text-xs font-medium text-gray-600 mb-1">
            SEEDS <span className="text-gray-400">(empty = demo)</span>
          </label>
          <textarea
            value={seedsText}
            onChange={(e) => setSeedsText(e.target.value)}
            disabled={loading}
            rows={6}
            placeholder="technical_assets.json の内容..."
            className="w-full text-xs border border-gray-300 rounded-md px-2 py-1.5 resize-y font-mono"
          />
        </div>

        {/* LLM mode: hypothesis count slider */}
        {!isManual && (
          <div>
            <label className="block text-xs font-medium text-gray-600 mb-1">仮説数</label>
            <div className="flex items-center gap-2">
              <input
                type="range"
                min={1}
                max={10}
                value={hypothesisCount}
                onChange={(e) => setHypothesisCount(Number(e.target.value))}
                className="flex-1"
                disabled={loading}
              />
              <span className="text-sm font-mono text-gray-900 w-6 text-center">{hypothesisCount}</span>
            </div>
          </div>
        )}

        {/* Manual mode: hypothesis titles */}
        {isManual && (
          <div>
            <label className="block text-xs font-medium text-gray-600 mb-1">仮説タイトル</label>
            <div className="space-y-2">
              {manualHypotheses.map((title, i) => (
                <div key={i} className="flex items-center gap-1">
                  <input
                    type="text"
                    value={title}
                    onChange={(e) => updateManualHypothesis(i, e.target.value)}
                    disabled={loading}
                    placeholder={`仮説 ${i + 1}`}
                    className="flex-1 text-xs border border-gray-300 rounded px-2 py-1"
                  />
                  {manualHypotheses.length > 1 && (
                    <button
                      onClick={() => removeManualHypothesis(i)}
                      disabled={loading}
                      className="text-gray-400 hover:text-red-500 text-xs px-1"
                    >
                      x
                    </button>
                  )}
                </div>
              ))}
              <button
                onClick={addManualHypothesis}
                disabled={loading}
                className="text-xs text-blue-600 hover:text-blue-800"
              >
                + 追加
              </button>
            </div>
          </div>
        )}

        <hr className="border-gray-200" />

        {/* Run / Cancel */}
        {!loading ? (
          <button
            onClick={handleStart}
            className="w-full px-4 py-2 bg-gray-900 text-white text-sm rounded-lg hover:bg-gray-800"
          >
            Run Pipeline
          </button>
        ) : (
          <button
            onClick={handleCancel}
            className="w-full px-4 py-2 bg-red-600 text-white text-sm rounded-lg hover:bg-red-700"
          >
            Cancel
          </button>
        )}
      </div>

      {/* Main Area */}
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-4xl mx-auto px-6 py-8 space-y-6">
          {/* Error */}
          {error && (
            <div className="px-4 py-3 bg-red-50 border border-red-200 rounded-lg">
              <p className="text-sm text-red-700">{error}</p>
            </div>
          )}

          {/* Step progress */}
          {(currentStep != null || completedSteps.size > 0) && (
            <div className="space-y-3">
              {[1, 2, 3].map((step) => {
                const isActive = currentStep === step;
                const completed = completedSteps.get(step);
                const isSkipped = isManual && (step === 1 || step === 2);
                const stepLabels: Record<number, string> = {
                  1: 'Deep Research: 仮説生成',
                  2: '構造化出力: JSON抽出',
                  3: 'Deep Research: 深掘り並列実行',
                };

                return (
                  <div
                    key={step}
                    className={`flex items-center gap-3 px-4 py-3 rounded-lg border ${
                      isSkipped
                        ? 'border-gray-200 bg-gray-100 opacity-50'
                        : isActive
                        ? 'border-blue-300 bg-blue-50'
                        : completed
                        ? 'border-green-200 bg-green-50'
                        : 'border-gray-200 bg-gray-50'
                    }`}
                  >
                    <div
                      className={`w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold ${
                        isSkipped
                          ? 'bg-gray-300 text-gray-500'
                          : isActive
                          ? 'bg-blue-600 text-white'
                          : completed
                          ? 'bg-green-600 text-white'
                          : 'bg-gray-300 text-gray-600'
                      }`}
                    >
                      {isSkipped ? '\u2014' : completed ? '\u2713' : step}
                    </div>
                    <div className="flex-1">
                      <div className={`text-sm font-medium ${isSkipped ? 'text-gray-400' : 'text-gray-900'}`}>
                        STEP {step}: {stepLabels[step]}
                        {isSkipped && <span className="ml-2 text-xs text-gray-400">(skipped)</span>}
                      </div>
                      {isActive && stepDescription && (
                        <div className="text-xs text-blue-600 mt-0.5 flex items-center gap-1">
                          <span className="inline-block w-3 h-3 border-2 border-blue-600 border-t-transparent rounded-full animate-spin" />
                          {stepDescription}
                        </div>
                      )}
                      {completed && (
                        <div className="text-xs text-green-700 mt-0.5">{completed}</div>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {/* STEP 1 result (collapsible) */}
          {step1Text && (
            <div className="border border-gray-200 rounded-lg bg-white">
              <div className="flex items-center justify-between px-4 py-3">
                <button
                  onClick={() => setStep1Open(!step1Open)}
                  className="flex-1 text-left text-sm font-medium text-gray-700 hover:text-gray-900"
                >
                  <span>STEP 1: 仮説生成レポート ({step1Text.length.toLocaleString()} chars)</span>
                  <span className="text-gray-400 ml-2">{step1Open ? '\u25B2' : '\u25BC'}</span>
                </button>
                <div className="flex items-center gap-2 ml-2">
                  <CopyButton text={step1Text} />
                  <DownloadButton filename="step1_report.md" content={step1Text} label="DL" />
                </div>
              </div>
              {step1Open && (
                <div className="px-6 pb-6 prose prose-sm max-w-none text-gray-800 border-t border-gray-100">
                  <Markdown>{step1Text}</Markdown>
                </div>
              )}
            </div>
          )}

          {/* STEP 2 hypotheses */}
          {hypotheses.length > 0 && (
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <h3 className="text-sm font-semibold text-gray-700">
                  {isManual ? 'Manual仮説' : 'STEP 2: 抽出された仮説'}
                </h3>
                <div className="flex items-center gap-2">
                  <button
                    onClick={() => setJsonViewOpen(!jsonViewOpen)}
                    className="text-xs text-gray-500 hover:text-gray-700 border border-gray-200 rounded px-2 py-1"
                  >
                    {jsonViewOpen ? 'Card View' : 'JSON View'}
                  </button>
                  {hypothesesJson && (
                    <CopyButton text={JSON.stringify(hypothesesJson, null, 2)} label="Copy JSON" />
                  )}
                </div>
              </div>

              {jsonViewOpen && hypothesesJson ? (
                <div className="border border-gray-200 rounded-lg bg-white p-4">
                  <pre className="text-xs text-gray-800 overflow-x-auto whitespace-pre-wrap font-mono">
                    {JSON.stringify(hypothesesJson, null, 2)}
                  </pre>
                </div>
              ) : (
                <div className="grid gap-3">
                  {hypotheses.map((h) => {
                    const isDetailOpen = hypothesisDetailOpen.has(h.index);
                    return (
                      <div
                        key={h.index}
                        className="border border-gray-200 rounded-lg bg-white"
                      >
                        <button
                          onClick={() => toggleHypothesisDetail(h.index)}
                          className="w-full flex items-center gap-4 px-4 py-3 hover:bg-gray-50 text-left"
                        >
                          <div className="w-8 h-8 rounded-full bg-indigo-100 text-indigo-700 flex items-center justify-center text-sm font-bold shrink-0">
                            {h.index + 1}
                          </div>
                          <div className="flex-1 min-w-0">
                            <div className="text-sm font-medium text-gray-900 truncate">{h.title}</div>
                            <div className="flex items-center gap-2 mt-0.5">
                              {h.cap_id_fingerprint && (
                                <span className="text-[10px] font-mono text-gray-400">{h.cap_id_fingerprint}</span>
                              )}
                              {h.verdict_tag && (
                                <span className={`text-[10px] px-1.5 py-0.5 rounded font-medium ${
                                  h.verdict_tag === 'GO' ? 'bg-green-100 text-green-700'
                                  : h.verdict_tag === 'PIVOT' ? 'bg-yellow-100 text-yellow-700'
                                  : 'bg-gray-100 text-gray-600'
                                }`}>
                                  {h.verdict_tag}
                                </span>
                              )}
                            </div>
                          </div>
                          <div className="text-xs font-mono text-gray-500 shrink-0">
                            {h.score > 0 ? `score: ${h.score.toFixed(2)}` : ''}
                          </div>
                          <span className="text-gray-400 text-xs shrink-0">
                            {isDetailOpen ? '\u25B2' : '\u25BC'}
                          </span>
                        </button>
                        {isDetailOpen && (
                          <div className="px-4 pb-3 border-t border-gray-100 space-y-2 text-xs text-gray-600">
                            {h.physical_contradiction && (
                              <div>
                                <span className="font-medium text-gray-700">Physical Contradiction: </span>
                                {h.physical_contradiction}
                              </div>
                            )}
                            {h.verdict_reason && (
                              <div>
                                <span className="font-medium text-gray-700">Verdict Reason: </span>
                                {h.verdict_reason}
                              </div>
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          )}

          {/* STEP 3 results (collapsible per hypothesis) */}
          {step3Results.length > 0 && (
            <div className="space-y-3">
              <h3 className="text-sm font-semibold text-gray-700">STEP 3: 深掘りレポート</h3>
              {step3Results.map((r, i) => (
                <div key={i} className={`border rounded-lg bg-white ${r.error ? 'border-red-200' : 'border-gray-200'}`}>
                  <div className="flex items-center justify-between px-4 py-3">
                    <button
                      onClick={() => !r.loading && toggleStep3(i)}
                      className={`flex-1 text-left text-sm font-medium hover:text-gray-900 ${r.loading ? 'cursor-default' : ''} ${r.error ? 'text-red-700' : 'text-gray-700'}`}
                    >
                      <span className="truncate flex items-center gap-2">
                        [{i + 1}] {r.title}
                        {r.loading && (
                          <span className="inline-block w-3 h-3 border-2 border-blue-600 border-t-transparent rounded-full animate-spin" />
                        )}
                      </span>
                      <span className="text-gray-400 ml-2">
                        {r.loading ? '' : r.error ? '\u2717' : step3Open.has(i) ? '\u25B2' : '\u25BC'}
                      </span>
                    </button>
                    {r.text && (
                      <div className="flex items-center gap-2 ml-2">
                        <CopyButton text={r.text} />
                        <DownloadButton filename={`step3_${i + 1}_${r.title.slice(0, 30).replace(/\s+/g, '_')}.md`} content={r.text} label="DL" />
                      </div>
                    )}
                  </div>
                  {r.error && (
                    <div className="px-4 pb-3 text-xs text-red-600 border-t border-red-100">
                      Error: {r.error}
                    </div>
                  )}
                  {!r.loading && r.text && step3Open.has(i) && (
                    <div className="px-6 pb-6 prose prose-sm max-w-none text-gray-800 border-t border-gray-100">
                      <Markdown>{r.text}</Markdown>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}

          {/* Done */}
          {done && (
            <div className="px-4 py-3 bg-green-50 border border-green-200 rounded-lg flex items-center justify-between">
              <p className="text-sm text-green-700">
                Pipeline complete. {hypotheses.length}件の仮説{isManual ? '' : 'を生成し'}、各深掘りレポートを作成しました。
              </p>
              <DownloadButton
                filename="pipeline_results.md"
                content={buildAllResultsMarkdown()}
                label="Download All"
              />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
