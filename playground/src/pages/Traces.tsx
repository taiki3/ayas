import { useState, useEffect, useCallback } from 'react';
import {
  queryRuns,
  getRun,
  getTrace,
  type RunSummary,
  type RunDto,
  type RunFilterRequest,
} from '../lib/api';

type RunType = '' | 'chain' | 'llm' | 'tool' | 'retriever' | 'graph';
type RunStatus = '' | 'running' | 'success' | 'error';

function statusBadge(status: string) {
  switch (status) {
    case 'success':
      return 'bg-green-100 text-green-700';
    case 'error':
      return 'bg-red-100 text-red-700';
    case 'running':
      return 'bg-blue-100 text-blue-700';
    default:
      return 'bg-muted text-card-foreground';
  }
}

function formatLatency(ms: number | null): string {
  if (ms === null) return '-';
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

function formatTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

export default function Traces() {
  const [project, setProject] = useState('default');
  const [runTypeFilter, setRunTypeFilter] = useState<RunType>('');
  const [statusFilter, setStatusFilter] = useState<RunStatus>('');
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Detail panel
  const [selectedRun, setSelectedRun] = useState<RunDto | null>(null);
  const [traceRuns, setTraceRuns] = useState<RunDto[]>([]);
  const [detailLoading, setDetailLoading] = useState(false);

  const fetchRuns = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const filter: RunFilterRequest = { project, limit: 100 };
      if (runTypeFilter) filter.run_type = runTypeFilter;
      if (statusFilter) filter.status = statusFilter;
      const result = await queryRuns(filter);
      setRuns(result);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch runs');
    } finally {
      setLoading(false);
    }
  }, [project, runTypeFilter, statusFilter]);

  useEffect(() => {
    fetchRuns();
  }, [fetchRuns]);

  const handleSelectRun = async (run: RunSummary) => {
    setDetailLoading(true);
    try {
      const [detail, trace] = await Promise.all([
        getRun(run.run_id, run.project),
        getTrace(run.run_id, run.project).catch(() => [] as RunDto[]),
      ]);
      setSelectedRun(detail);
      // If the trace only contains the run itself, try using trace_id
      if (trace.length <= 1 && detail.trace_id) {
        const traceResult = await getTrace(detail.trace_id, run.project).catch(() => [] as RunDto[]);
        setTraceRuns(traceResult);
      } else {
        setTraceRuns(trace);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch run detail');
    } finally {
      setDetailLoading(false);
    }
  };

  return (
    <div className="flex h-full">
      {/* Run List */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Filters */}
        <div className="flex items-center gap-3 px-6 py-3 border-b border-border bg-card shrink-0">
          <div className="flex items-center gap-1.5">
            <label className="text-xs font-medium text-muted-foreground uppercase">Project</label>
            <input
              type="text"
              value={project}
              onChange={(e) => setProject(e.target.value)}
              className="px-2 py-1.5 border border-border rounded-md text-sm w-32 focus:outline-none focus:ring-1 focus:ring-ring"
            />
          </div>
          <div className="flex items-center gap-1.5">
            <label className="text-xs font-medium text-muted-foreground uppercase">Type</label>
            <select
              value={runTypeFilter}
              onChange={(e) => setRunTypeFilter(e.target.value as RunType)}
              className="px-2 py-1.5 border border-border rounded-md text-sm bg-card"
            >
              <option value="">All</option>
              <option value="chain">Chain</option>
              <option value="llm">LLM</option>
              <option value="tool">Tool</option>
              <option value="retriever">Retriever</option>
              <option value="graph">Graph</option>
            </select>
          </div>
          <div className="flex items-center gap-1.5">
            <label className="text-xs font-medium text-muted-foreground uppercase">Status</label>
            <select
              value={statusFilter}
              onChange={(e) => setStatusFilter(e.target.value as RunStatus)}
              className="px-2 py-1.5 border border-border rounded-md text-sm bg-card"
            >
              <option value="">All</option>
              <option value="success">Success</option>
              <option value="error">Error</option>
              <option value="running">Running</option>
            </select>
          </div>
          <button
            onClick={fetchRuns}
            disabled={loading}
            className="px-3 py-1.5 bg-primary text-primary-foreground text-sm rounded-md hover:opacity-90 disabled:opacity-50"
          >
            {loading ? 'Loading...' : 'Refresh'}
          </button>
        </div>

        {error && (
          <div className="px-6 py-3 bg-destructive/10 border-b border-destructive/30">
            <p className="text-sm text-destructive">{error}</p>
          </div>
        )}

        {/* Table */}
        <div className="flex-1 overflow-auto">
          <table className="w-full text-sm">
            <thead className="bg-surface sticky top-0">
              <tr className="text-left text-xs font-medium text-muted-foreground uppercase tracking-wide">
                <th className="px-6 py-3">Name</th>
                <th className="px-4 py-3">Type</th>
                <th className="px-4 py-3">Status</th>
                <th className="px-4 py-3">Latency</th>
                <th className="px-4 py-3">Start Time</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {runs.length === 0 && !loading && (
                <tr>
                  <td colSpan={5} className="px-6 py-12 text-center text-muted-foreground">
                    No runs found. Try adjusting the filters or ingesting trace data.
                  </td>
                </tr>
              )}
              {runs.map((run) => (
                <tr
                  key={run.run_id}
                  onClick={() => handleSelectRun(run)}
                  className={`cursor-pointer hover:bg-surface transition-colors ${
                    selectedRun?.run_id === run.run_id ? 'bg-blue-50' : ''
                  }`}
                >
                  <td className="px-6 py-3 font-medium text-foreground truncate max-w-[200px]">{run.name}</td>
                  <td className="px-4 py-3">
                    <span className="px-2 py-0.5 bg-muted text-card-foreground rounded text-xs">{run.run_type}</span>
                  </td>
                  <td className="px-4 py-3">
                    <span className={`px-2 py-0.5 rounded text-xs font-medium ${statusBadge(run.status)}`}>
                      {run.status}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-card-foreground">{formatLatency(run.latency_ms)}</td>
                  <td className="px-4 py-3 text-muted-foreground text-xs">{formatTime(run.start_time)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* Detail Panel */}
      <aside className="w-[400px] border-l border-border bg-card overflow-y-auto shrink-0">
        {detailLoading && (
          <div className="flex items-center justify-center h-full text-muted-foreground text-sm">Loading...</div>
        )}
        {!detailLoading && !selectedRun && (
          <div className="flex items-center justify-center h-full text-muted-foreground text-sm">
            Select a run to view details
          </div>
        )}
        {!detailLoading && selectedRun && (
          <div className="p-4 space-y-4">
            <div>
              <h3 className="text-sm font-semibold text-foreground mb-1">{selectedRun.name}</h3>
              <div className="flex gap-2 text-xs">
                <span className={`px-2 py-0.5 rounded font-medium ${statusBadge(selectedRun.status)}`}>
                  {selectedRun.status}
                </span>
                <span className="px-2 py-0.5 bg-muted text-card-foreground rounded">{selectedRun.run_type}</span>
              </div>
            </div>

            <div className="grid grid-cols-2 gap-3 text-xs">
              <div>
                <span className="text-muted-foreground block">Run ID</span>
                <span className="text-foreground font-mono truncate block">{selectedRun.run_id}</span>
              </div>
              <div>
                <span className="text-muted-foreground block">Trace ID</span>
                <span className="text-foreground font-mono truncate block">{selectedRun.trace_id || '-'}</span>
              </div>
              <div>
                <span className="text-muted-foreground block">Latency</span>
                <span className="text-foreground">{formatLatency(selectedRun.latency_ms)}</span>
              </div>
              <div>
                <span className="text-muted-foreground block">Tokens</span>
                <span className="text-foreground">
                  {selectedRun.total_tokens !== null ? selectedRun.total_tokens : '-'}
                </span>
              </div>
              <div>
                <span className="text-muted-foreground block">Start</span>
                <span className="text-foreground">{formatTime(selectedRun.start_time)}</span>
              </div>
              <div>
                <span className="text-muted-foreground block">End</span>
                <span className="text-foreground">{selectedRun.end_time ? formatTime(selectedRun.end_time) : '-'}</span>
              </div>
            </div>

            {/* Input */}
            <div>
              <h4 className="text-xs font-medium text-muted-foreground uppercase mb-1">Input</h4>
              <pre className="text-xs bg-surface rounded p-2 overflow-x-auto text-card-foreground max-h-40 overflow-y-auto font-mono">
                {tryFormatJson(selectedRun.input)}
              </pre>
            </div>

            {/* Output */}
            {selectedRun.output && (
              <div>
                <h4 className="text-xs font-medium text-muted-foreground uppercase mb-1">Output</h4>
                <pre className="text-xs bg-surface rounded p-2 overflow-x-auto text-card-foreground max-h-40 overflow-y-auto font-mono">
                  {tryFormatJson(selectedRun.output)}
                </pre>
              </div>
            )}

            {/* Error */}
            {selectedRun.error && (
              <div>
                <h4 className="text-xs font-medium text-muted-foreground uppercase mb-1">Error</h4>
                <pre className="text-xs bg-destructive/10 rounded p-2 overflow-x-auto text-destructive max-h-40 overflow-y-auto font-mono">
                  {selectedRun.error}
                </pre>
              </div>
            )}

            {/* Metadata */}
            <div>
              <h4 className="text-xs font-medium text-muted-foreground uppercase mb-1">Metadata</h4>
              <pre className="text-xs bg-surface rounded p-2 overflow-x-auto text-card-foreground max-h-32 overflow-y-auto font-mono">
                {tryFormatJson(selectedRun.metadata)}
              </pre>
            </div>

            {/* Trace Tree */}
            {traceRuns.length > 1 && (
              <div>
                <h4 className="text-xs font-medium text-muted-foreground uppercase mb-2">
                  Trace Tree ({traceRuns.length} runs)
                </h4>
                <TraceTree
                  runs={traceRuns}
                  selectedRunId={selectedRun.run_id}
                  onSelect={(r) => setSelectedRun(r)}
                />
              </div>
            )}
          </div>
        )}
      </aside>
    </div>
  );
}

function tryFormatJson(s: string): string {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return s;
  }
}

function TraceTree({
  runs,
  selectedRunId,
  onSelect,
}: {
  runs: RunDto[];
  selectedRunId: string;
  onSelect: (run: RunDto) => void;
}) {
  // Build parent->children map
  const childrenMap = new Map<string | null, RunDto[]>();
  for (const run of runs) {
    const parent = run.parent_run_id || null;
    if (!childrenMap.has(parent)) childrenMap.set(parent, []);
    childrenMap.get(parent)!.push(run);
  }

  // Find root runs (no parent or parent not in the run set)
  const runIdSet = new Set(runs.map((r) => r.run_id));
  const roots = runs.filter((r) => !r.parent_run_id || !runIdSet.has(r.parent_run_id));

  function renderNode(run: RunDto, depth: number): React.ReactNode {
    const children = childrenMap.get(run.run_id) || [];
    const isSelected = run.run_id === selectedRunId;

    return (
      <div key={run.run_id}>
        <div
          onClick={() => onSelect(run)}
          style={{ paddingLeft: `${depth * 16 + 8}px` }}
          className={`flex items-center gap-2 py-1.5 cursor-pointer rounded text-xs hover:bg-muted ${
            isSelected ? 'bg-blue-50 font-medium' : ''
          }`}
        >
          <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${
            run.status === 'success' ? 'bg-green-500' :
            run.status === 'error' ? 'bg-red-500' :
            'bg-blue-500'
          }`} />
          <span className="text-card-foreground truncate">{run.name}</span>
          <span className="text-muted-foreground ml-auto shrink-0">{run.run_type}</span>
        </div>
        {children.map((child) => renderNode(child, depth + 1))}
      </div>
    );
  }

  return <div className="border border-border rounded-lg p-1">{roots.map((r) => renderNode(r, 0))}</div>;
}
