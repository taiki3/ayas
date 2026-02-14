import { useState, useEffect, useCallback } from 'react';
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  BarChart,
  Bar,
} from 'recharts';
import {
  queryRuns,
  getStats,
  type RunSummary,
  type StatsResponse,
} from '../lib/api';

function formatLatency(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

export default function Dashboard() {
  const [project, setProject] = useState('default');
  const [stats, setStats] = useState<StatsResponse | null>(null);
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [statsRes, runsRes] = await Promise.all([
        getStats(project),
        queryRuns({ project, limit: 200 }),
      ]);
      setStats(statsRes);
      setRuns(runsRes);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load dashboard data');
    } finally {
      setLoading(false);
    }
  }, [project]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Compute derived metrics
  const totalRuns = stats?.tokens.run_count ?? runs.length;
  const successRuns = runs.filter((r) => r.status === 'success').length;
  const errorRuns = runs.filter((r) => r.status === 'error').length;
  const successRate = totalRuns > 0 ? ((successRuns / totalRuns) * 100).toFixed(1) : '0.0';
  const avgLatency = runs.length > 0
    ? runs.filter((r) => r.latency_ms !== null).reduce((sum, r) => sum + (r.latency_ms ?? 0), 0) /
      Math.max(1, runs.filter((r) => r.latency_ms !== null).length)
    : 0;

  // Time-series: group runs by hour
  const timeSeriesData = buildTimeSeries(runs);

  // Run type distribution
  const typeDistribution = buildTypeDistribution(runs);

  return (
    <div className="h-full overflow-y-auto bg-surface">
      <div className="max-w-6xl mx-auto px-6 py-6 space-y-6">
        {/* Header */}
        <div className="flex items-center justify-between">
          <h2 className="text-lg font-semibold text-foreground">Dashboard</h2>
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-1.5">
              <label className="text-xs font-medium text-muted-foreground uppercase">Project</label>
              <input
                type="text"
                value={project}
                onChange={(e) => setProject(e.target.value)}
                className="px-2 py-1.5 border border-border rounded-md text-sm w-32 focus:outline-none focus:ring-1 focus:ring-ring"
              />
            </div>
            <button
              onClick={refresh}
              disabled={loading}
              className="px-3 py-1.5 bg-primary text-primary-foreground text-sm rounded-md hover:opacity-90 disabled:opacity-50"
            >
              {loading ? 'Loading...' : 'Refresh'}
            </button>
          </div>
        </div>

        {error && (
          <div className="px-4 py-3 bg-destructive/10 border border-destructive/30 rounded-lg">
            <p className="text-sm text-destructive">{error}</p>
          </div>
        )}

        {/* Summary Cards */}
        <div className="grid grid-cols-4 gap-4">
          <SummaryCard
            label="Total Runs"
            value={String(totalRuns)}
          />
          <SummaryCard
            label="Success Rate"
            value={`${successRate}%`}
            sub={`${successRuns} ok / ${errorRuns} err`}
            color={parseFloat(successRate) >= 90 ? 'green' : parseFloat(successRate) >= 70 ? 'yellow' : 'red'}
          />
          <SummaryCard
            label="Avg Latency"
            value={formatLatency(avgLatency)}
          />
          <SummaryCard
            label="Total Tokens"
            value={stats ? String(stats.tokens.total_tokens) : '-'}
            sub={stats ? `${stats.tokens.total_input_tokens} in / ${stats.tokens.total_output_tokens} out` : ''}
          />
        </div>

        {/* Latency Percentiles */}
        {stats && (
          <div className="bg-card border border-border rounded-lg p-4">
            <h3 className="text-xs font-medium text-muted-foreground uppercase mb-3">Latency Percentiles</h3>
            <div className="grid grid-cols-4 gap-4">
              <div>
                <span className="text-xs text-muted-foreground">P50</span>
                <span className="block text-lg font-semibold text-foreground">{formatLatency(stats.latency.p50)}</span>
              </div>
              <div>
                <span className="text-xs text-muted-foreground">P90</span>
                <span className="block text-lg font-semibold text-foreground">{formatLatency(stats.latency.p90)}</span>
              </div>
              <div>
                <span className="text-xs text-muted-foreground">P95</span>
                <span className="block text-lg font-semibold text-foreground">{formatLatency(stats.latency.p95)}</span>
              </div>
              <div>
                <span className="text-xs text-muted-foreground">P99</span>
                <span className="block text-lg font-semibold text-foreground">{formatLatency(stats.latency.p99)}</span>
              </div>
            </div>
          </div>
        )}

        {/* Charts */}
        <div className="grid grid-cols-2 gap-4">
          {/* Time-series chart */}
          <div className="bg-card border border-border rounded-lg p-4">
            <h3 className="text-xs font-medium text-muted-foreground uppercase mb-3">Runs Over Time</h3>
            {timeSeriesData.length === 0 ? (
              <p className="text-sm text-muted-foreground text-center py-8">No data available</p>
            ) : (
              <ResponsiveContainer width="100%" height={220}>
                <LineChart data={timeSeriesData}>
                  <CartesianGrid strokeDasharray="3 3" stroke="#f0f0f0" />
                  <XAxis dataKey="label" tick={{ fontSize: 11 }} />
                  <YAxis tick={{ fontSize: 11 }} allowDecimals={false} />
                  <Tooltip />
                  <Line type="monotone" dataKey="total" stroke="#111827" strokeWidth={2} dot={false} />
                  <Line type="monotone" dataKey="success" stroke="#16a34a" strokeWidth={1.5} dot={false} />
                  <Line type="monotone" dataKey="error" stroke="#dc2626" strokeWidth={1.5} dot={false} />
                </LineChart>
              </ResponsiveContainer>
            )}
          </div>

          {/* Type distribution */}
          <div className="bg-card border border-border rounded-lg p-4">
            <h3 className="text-xs font-medium text-muted-foreground uppercase mb-3">Run Type Distribution</h3>
            {typeDistribution.length === 0 ? (
              <p className="text-sm text-muted-foreground text-center py-8">No data available</p>
            ) : (
              <ResponsiveContainer width="100%" height={220}>
                <BarChart data={typeDistribution}>
                  <CartesianGrid strokeDasharray="3 3" stroke="#f0f0f0" />
                  <XAxis dataKey="type" tick={{ fontSize: 11 }} />
                  <YAxis tick={{ fontSize: 11 }} allowDecimals={false} />
                  <Tooltip />
                  <Bar dataKey="count" fill="#111827" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function SummaryCard({
  label,
  value,
  sub,
  color,
}: {
  label: string;
  value: string;
  sub?: string;
  color?: 'green' | 'yellow' | 'red';
}) {
  const colorClass = color === 'green'
    ? 'text-green-600'
    : color === 'yellow'
      ? 'text-yellow-600'
      : color === 'red'
        ? 'text-red-600'
        : 'text-foreground';

  return (
    <div className="bg-card border border-border rounded-lg p-4">
      <span className="text-xs font-medium text-muted-foreground uppercase">{label}</span>
      <div className={`text-2xl font-semibold mt-1 ${colorClass}`}>{value}</div>
      {sub && <p className="text-xs text-muted-foreground mt-0.5">{sub}</p>}
    </div>
  );
}

function buildTimeSeries(runs: RunSummary[]) {
  if (runs.length === 0) return [];

  const buckets = new Map<string, { total: number; success: number; error: number }>();

  for (const run of runs) {
    const date = new Date(run.start_time);
    const key = `${date.getMonth() + 1}/${date.getDate()} ${String(date.getHours()).padStart(2, '0')}:00`;
    if (!buckets.has(key)) buckets.set(key, { total: 0, success: 0, error: 0 });
    const bucket = buckets.get(key)!;
    bucket.total++;
    if (run.status === 'success') bucket.success++;
    if (run.status === 'error') bucket.error++;
  }

  return [...buckets.entries()]
    .map(([label, data]) => ({ label, ...data }));
}

function buildTypeDistribution(runs: RunSummary[]) {
  const counts = new Map<string, number>();
  for (const run of runs) {
    counts.set(run.run_type, (counts.get(run.run_type) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([type, count]) => ({ type, count }))
    .sort((a, b) => b.count - a.count);
}
