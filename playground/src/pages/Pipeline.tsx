import { useState, useRef } from 'react';
import Markdown from 'react-markdown';
import { pipelineInvokeStream, getApiKey, type PipelineSseEvent } from '../lib/api';

interface HypothesisCard {
  index: number;
  title: string;
  score: number;
}

interface Step3Report {
  title: string;
  text: string;
}

export default function Pipeline() {
  const [hypothesisCount, setHypothesisCount] = useState(3);
  const [loading, setLoading] = useState(false);
  const [currentStep, setCurrentStep] = useState<number | null>(null);
  const [stepDescription, setStepDescription] = useState<string | null>(null);
  const [completedSteps, setCompletedSteps] = useState<Map<number, string>>(new Map());
  const [hypotheses, setHypotheses] = useState<HypothesisCard[]>([]);
  const [step1Text, setStep1Text] = useState<string | null>(null);
  const [step1Open, setStep1Open] = useState(false);
  const [step3Results, setStep3Results] = useState<Step3Report[]>([]);
  const [step3Open, setStep3Open] = useState<Set<number>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const handleStart = async () => {
    if (loading) return;

    if (!getApiKey('gemini')) {
      setError('Gemini API key is required. Click "API Keys" in the header to configure.');
      return;
    }

    setError(null);
    setLoading(true);
    setCurrentStep(null);
    setStepDescription(null);
    setCompletedSteps(new Map());
    setHypotheses([]);
    setStep1Text(null);
    setStep1Open(false);
    setStep3Results([]);
    setStep3Open(new Set());
    setDone(false);

    const controller = new AbortController();
    abortRef.current = controller;

    try {
      await pipelineInvokeStream(
        hypothesisCount,
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
                  { index: event.index ?? prev.length, title: event.title!, score: event.score ?? 0 },
                ]);
              }
              break;
            case 'complete':
              setStep1Text(event.step1_text ?? null);
              setStep3Results(event.step3_results ?? []);
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
      if (next.has(index)) {
        next.delete(index);
      } else {
        next.add(index);
      }
      return next;
    });
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="max-w-4xl mx-auto px-6 py-8 space-y-6">
        {/* Controls */}
        <div className="bg-white border border-gray-200 rounded-lg p-6 space-y-4">
          <h2 className="text-lg font-semibold text-gray-900">Hypothesis Pipeline</h2>
          <p className="text-sm text-gray-500">
            Deep Research + 構造化出力による3ステップ仮説生成パイプライン
          </p>

          <div className="flex items-center gap-6">
            <div className="flex items-center gap-3">
              <label className="text-sm text-gray-600">仮説数:</label>
              <input
                type="range"
                min={1}
                max={10}
                value={hypothesisCount}
                onChange={(e) => setHypothesisCount(Number(e.target.value))}
                className="w-32"
                disabled={loading}
              />
              <span className="text-sm font-mono text-gray-900 w-6 text-center">{hypothesisCount}</span>
            </div>

            {!loading ? (
              <button
                onClick={handleStart}
                className="px-6 py-2 bg-gray-900 text-white text-sm rounded-lg hover:bg-gray-800 disabled:opacity-40 disabled:cursor-not-allowed"
              >
                Run Pipeline
              </button>
            ) : (
              <button
                onClick={handleCancel}
                className="px-6 py-2 bg-red-600 text-white text-sm rounded-lg hover:bg-red-700"
              >
                Cancel
              </button>
            )}
          </div>
        </div>

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
              const stepLabels: Record<number, string> = {
                1: 'Deep Research: 仮説生成',
                2: '構造化出力: JSON抽出',
                3: 'Deep Research: 深掘り並列実行',
              };

              return (
                <div
                  key={step}
                  className={`flex items-center gap-3 px-4 py-3 rounded-lg border ${
                    isActive
                      ? 'border-blue-300 bg-blue-50'
                      : completed
                      ? 'border-green-200 bg-green-50'
                      : 'border-gray-200 bg-gray-50'
                  }`}
                >
                  <div
                    className={`w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold ${
                      isActive
                        ? 'bg-blue-600 text-white'
                        : completed
                        ? 'bg-green-600 text-white'
                        : 'bg-gray-300 text-gray-600'
                    }`}
                  >
                    {completed ? '\u2713' : step}
                  </div>
                  <div className="flex-1">
                    <div className="text-sm font-medium text-gray-900">
                      STEP {step}: {stepLabels[step]}
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
            <button
              onClick={() => setStep1Open(!step1Open)}
              className="w-full flex items-center justify-between px-4 py-3 text-sm font-medium text-gray-700 hover:bg-gray-50"
            >
              <span>STEP 1: 仮説生成レポート ({step1Text.length.toLocaleString()} chars)</span>
              <span className="text-gray-400">{step1Open ? '\u25B2' : '\u25BC'}</span>
            </button>
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
            <h3 className="text-sm font-semibold text-gray-700">STEP 2: 抽出された仮説</h3>
            <div className="grid gap-3">
              {hypotheses.map((h) => (
                <div
                  key={h.index}
                  className="flex items-center gap-4 px-4 py-3 border border-gray-200 rounded-lg bg-white"
                >
                  <div className="w-8 h-8 rounded-full bg-indigo-100 text-indigo-700 flex items-center justify-center text-sm font-bold">
                    {h.index + 1}
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="text-sm font-medium text-gray-900 truncate">{h.title}</div>
                  </div>
                  <div className="text-xs font-mono text-gray-500 shrink-0">
                    score: {h.score.toFixed(2)}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* STEP 3 results (collapsible per hypothesis) */}
        {step3Results.length > 0 && (
          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-gray-700">STEP 3: 深掘りレポート</h3>
            {step3Results.map((r, i) => (
              <div key={i} className="border border-gray-200 rounded-lg bg-white">
                <button
                  onClick={() => toggleStep3(i)}
                  className="w-full flex items-center justify-between px-4 py-3 text-sm font-medium text-gray-700 hover:bg-gray-50"
                >
                  <span className="truncate">
                    [{i + 1}] {r.title}
                  </span>
                  <span className="text-gray-400 shrink-0 ml-2">
                    {step3Open.has(i) ? '\u25B2' : '\u25BC'}
                  </span>
                </button>
                {step3Open.has(i) && (
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
          <div className="px-4 py-3 bg-green-50 border border-green-200 rounded-lg">
            <p className="text-sm text-green-700">
              Pipeline complete. {hypotheses.length}件の仮説を生成し、各深掘りレポートを作成しました。
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
