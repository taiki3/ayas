import { useState, useCallback } from 'react';
import {
  getTrace,
  type RunDto,
} from '../lib/api';

function formatTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function tryFormatJson(s: string): string {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return s;
  }
}

interface StepSnapshot {
  run: RunDto;
  stepNumber: number;
}

export default function TimeTravel() {
  const [traceId, setTraceId] = useState('');
  const [project, setProject] = useState('default');
  const [steps, setSteps] = useState<StepSnapshot[]>([]);
  const [selectedStep, setSelectedStep] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchTrace = useCallback(async () => {
    if (!traceId.trim()) return;
    setLoading(true);
    setError(null);
    setSelectedStep(null);
    try {
      const runs = await getTrace(traceId.trim(), project);
      if (runs.length === 0) {
        setError('No runs found for this trace ID.');
        setSteps([]);
        return;
      }
      // Sort by start_time to establish execution order
      const sorted = [...runs].sort(
        (a, b) => new Date(a.start_time).getTime() - new Date(b.start_time).getTime(),
      );
      const snapshots: StepSnapshot[] = sorted.map((run, i) => ({
        run,
        stepNumber: i + 1,
      }));
      setSteps(snapshots);
      if (snapshots.length > 0) setSelectedStep(0);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch trace');
      setSteps([]);
    } finally {
      setLoading(false);
    }
  }, [traceId, project]);

  const currentStep = selectedStep !== null ? steps[selectedStep] : null;
  const prevStep = selectedStep !== null && selectedStep > 0 ? steps[selectedStep - 1] : null;

  return (
    <div className="flex h-full">
      {/* Left: Steps Timeline */}
      <div className="w-[360px] border-r border-gray-200 bg-white flex flex-col shrink-0">
        <div className="p-4 border-b border-gray-200 space-y-3">
          <div>
            <label className="block text-xs font-medium text-gray-500 uppercase mb-1">Trace ID</label>
            <input
              type="text"
              value={traceId}
              onChange={(e) => setTraceId(e.target.value)}
              placeholder="Enter trace UUID..."
              className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm font-mono focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
          </div>
          <div className="flex gap-2">
            <div className="flex-1">
              <label className="block text-xs font-medium text-gray-500 uppercase mb-1">Project</label>
              <input
                type="text"
                value={project}
                onChange={(e) => setProject(e.target.value)}
                className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-1 focus:ring-blue-500"
              />
            </div>
            <button
              onClick={fetchTrace}
              disabled={loading || !traceId.trim()}
              className="self-end px-4 py-2 bg-gray-900 text-white text-sm rounded-md hover:bg-gray-800 disabled:opacity-50"
            >
              {loading ? 'Loading...' : 'Load'}
            </button>
          </div>
        </div>

        {error && (
          <div className="px-4 py-3 bg-red-50 border-b border-red-200">
            <p className="text-sm text-red-700">{error}</p>
          </div>
        )}

        {/* Steps list */}
        <div className="flex-1 overflow-y-auto p-3">
          {steps.length === 0 && !loading && (
            <p className="text-sm text-gray-400 text-center mt-8">
              Enter a Trace ID to load execution steps.
            </p>
          )}
          <div className="space-y-1">
            {steps.map((step, i) => (
              <div
                key={step.run.run_id}
                onClick={() => setSelectedStep(i)}
                className={`flex items-center gap-3 px-3 py-2.5 rounded-lg cursor-pointer transition-colors ${
                  selectedStep === i
                    ? 'bg-blue-50 border border-blue-200'
                    : 'hover:bg-gray-50 border border-transparent'
                }`}
              >
                {/* Step number */}
                <div className="flex flex-col items-center">
                  <span className={`w-7 h-7 rounded-full flex items-center justify-center text-xs font-medium ${
                    step.run.status === 'success'
                      ? 'bg-green-100 text-green-700'
                      : step.run.status === 'error'
                        ? 'bg-red-100 text-red-700'
                        : 'bg-blue-100 text-blue-700'
                  }`}>
                    {step.stepNumber}
                  </span>
                  {i < steps.length - 1 && (
                    <div className="w-px h-4 bg-gray-200 mt-1" />
                  )}
                </div>
                {/* Info */}
                <div className="flex-1 min-w-0">
                  <div className="text-sm font-medium text-gray-900 truncate">{step.run.name}</div>
                  <div className="flex gap-2 mt-0.5">
                    <span className="text-xs text-gray-500">{step.run.run_type}</span>
                    {step.run.latency_ms !== null && (
                      <span className="text-xs text-gray-400">{step.run.latency_ms}ms</span>
                    )}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* Right: Step Detail */}
      <div className="flex-1 overflow-y-auto bg-gray-50 p-6">
        {!currentStep && (
          <div className="flex items-center justify-center h-full text-gray-400 text-sm">
            Select a step to view its state snapshot
          </div>
        )}
        {currentStep && (
          <div className="max-w-3xl mx-auto space-y-6">
            <div className="flex items-center justify-between">
              <h2 className="text-lg font-semibold text-gray-900">
                Step {currentStep.stepNumber}: {currentStep.run.name}
              </h2>
              <div className="flex gap-2">
                <button
                  onClick={() => setSelectedStep(Math.max(0, (selectedStep ?? 0) - 1))}
                  disabled={selectedStep === 0}
                  className="px-3 py-1.5 text-sm border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-40"
                >
                  Prev
                </button>
                <button
                  onClick={() => setSelectedStep(Math.min(steps.length - 1, (selectedStep ?? 0) + 1))}
                  disabled={selectedStep === steps.length - 1}
                  className="px-3 py-1.5 text-sm border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-40"
                >
                  Next
                </button>
              </div>
            </div>

            {/* Metadata */}
            <div className="bg-white border border-gray-200 rounded-lg p-4">
              <div className="grid grid-cols-3 gap-4 text-xs">
                <div>
                  <span className="text-gray-500 block">Run ID</span>
                  <span className="text-gray-800 font-mono truncate block">{currentStep.run.run_id}</span>
                </div>
                <div>
                  <span className="text-gray-500 block">Type</span>
                  <span className="text-gray-800 capitalize">{currentStep.run.run_type}</span>
                </div>
                <div>
                  <span className="text-gray-500 block">Status</span>
                  <span className={`px-2 py-0.5 rounded font-medium ${
                    currentStep.run.status === 'success' ? 'bg-green-100 text-green-700' :
                    currentStep.run.status === 'error' ? 'bg-red-100 text-red-700' :
                    'bg-blue-100 text-blue-700'
                  }`}>{currentStep.run.status}</span>
                </div>
                <div>
                  <span className="text-gray-500 block">Start</span>
                  <span className="text-gray-800">{formatTime(currentStep.run.start_time)}</span>
                </div>
                <div>
                  <span className="text-gray-500 block">End</span>
                  <span className="text-gray-800">
                    {currentStep.run.end_time ? formatTime(currentStep.run.end_time) : '-'}
                  </span>
                </div>
                <div>
                  <span className="text-gray-500 block">Latency</span>
                  <span className="text-gray-800">
                    {currentStep.run.latency_ms !== null ? `${currentStep.run.latency_ms}ms` : '-'}
                  </span>
                </div>
              </div>
            </div>

            {/* Input Snapshot */}
            <div className="bg-white border border-gray-200 rounded-lg p-4">
              <h3 className="text-xs font-medium text-gray-500 uppercase mb-2">Input</h3>
              <pre className="text-xs bg-gray-50 rounded p-3 overflow-x-auto text-gray-700 max-h-60 overflow-y-auto font-mono">
                {tryFormatJson(currentStep.run.input)}
              </pre>
            </div>

            {/* Output Snapshot */}
            {currentStep.run.output && (
              <div className="bg-white border border-gray-200 rounded-lg p-4">
                <h3 className="text-xs font-medium text-gray-500 uppercase mb-2">Output</h3>
                <pre className="text-xs bg-gray-50 rounded p-3 overflow-x-auto text-gray-700 max-h-60 overflow-y-auto font-mono">
                  {tryFormatJson(currentStep.run.output)}
                </pre>
              </div>
            )}

            {/* Error */}
            {currentStep.run.error && (
              <div className="bg-white border border-red-200 rounded-lg p-4">
                <h3 className="text-xs font-medium text-red-500 uppercase mb-2">Error</h3>
                <pre className="text-xs bg-red-50 rounded p-3 overflow-x-auto text-red-700 max-h-60 overflow-y-auto font-mono">
                  {currentStep.run.error}
                </pre>
              </div>
            )}

            {/* Diff Highlight */}
            {prevStep && (
              <div className="bg-white border border-gray-200 rounded-lg p-4">
                <h3 className="text-xs font-medium text-gray-500 uppercase mb-2">
                  Changes from Step {prevStep.stepNumber}
                </h3>
                <DiffView
                  prevOutput={prevStep.run.output || prevStep.run.input}
                  currentInput={currentStep.run.input}
                />
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function DiffView({ prevOutput, currentInput }: { prevOutput: string; currentInput: string }) {
  const prevFormatted = tryFormatJson(prevOutput);
  const currentFormatted = tryFormatJson(currentInput);

  if (prevFormatted === currentFormatted) {
    return <p className="text-xs text-gray-400">No changes detected</p>;
  }

  const prevLines = prevFormatted.split('\n');
  const currentLines = currentFormatted.split('\n');
  const maxLines = Math.max(prevLines.length, currentLines.length);

  return (
    <div className="grid grid-cols-2 gap-2">
      <div>
        <span className="text-xs text-gray-400 block mb-1">Previous</span>
        <pre className="text-xs bg-red-50 rounded p-2 overflow-x-auto text-gray-700 max-h-40 overflow-y-auto font-mono">
          {prevLines.map((line, i) => (
            <div
              key={i}
              className={i < maxLines && line !== currentLines[i] ? 'bg-red-100' : ''}
            >
              {line}
            </div>
          ))}
        </pre>
      </div>
      <div>
        <span className="text-xs text-gray-400 block mb-1">Current</span>
        <pre className="text-xs bg-green-50 rounded p-2 overflow-x-auto text-gray-700 max-h-40 overflow-y-auto font-mono">
          {currentLines.map((line, i) => (
            <div
              key={i}
              className={i < maxLines && line !== prevLines[i] ? 'bg-green-100' : ''}
            >
              {line}
            </div>
          ))}
        </pre>
      </div>
    </div>
  );
}
