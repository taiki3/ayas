import { useState, useEffect, useCallback } from 'react';
import {
  queryRuns,
  queryFeedback,
  type RunSummary,
  type FeedbackDto,
} from '../lib/api';

export default function Projects() {
  // Projects are backed by the "project" field on runs
  const [projects, setProjects] = useState<string[]>([]);
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [feedback, setFeedback] = useState<FeedbackDto[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<'runs' | 'feedback'>('runs');

  // Discover projects by querying runs with no project filter
  const fetchProjects = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const allRuns = await queryRuns({ limit: 200 });
      const projectSet = new Set(allRuns.map((r) => r.project));
      const sorted = [...projectSet].sort();
      setProjects(sorted);
      if (sorted.length > 0 && !selectedProject) {
        setSelectedProject(sorted[0]);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load projects');
    } finally {
      setLoading(false);
    }
  }, [selectedProject]);

  useEffect(() => {
    fetchProjects();
  }, [fetchProjects]);

  // Load runs and feedback for selected project
  useEffect(() => {
    if (!selectedProject) return;
    const load = async () => {
      setLoading(true);
      try {
        const [projectRuns, fb] = await Promise.all([
          queryRuns({ project: selectedProject, limit: 50 }),
          queryFeedback().catch(() => [] as FeedbackDto[]),
        ]);
        setRuns(projectRuns);
        setFeedback(fb);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to load project data');
      } finally {
        setLoading(false);
      }
    };
    load();
  }, [selectedProject]);

  const successCount = runs.filter((r) => r.status === 'success').length;
  const errorCount = runs.filter((r) => r.status === 'error').length;

  return (
    <div className="flex h-full">
      {/* Project List */}
      <aside className="w-[240px] border-r border-gray-200 bg-white flex flex-col shrink-0">
        <div className="px-4 py-3 border-b border-gray-200">
          <h2 className="text-xs font-medium text-gray-500 uppercase tracking-wide">Projects</h2>
        </div>
        <div className="flex-1 overflow-y-auto p-2">
          {projects.length === 0 && !loading && (
            <p className="text-xs text-gray-400 text-center mt-4">No projects found</p>
          )}
          {projects.map((proj) => (
            <button
              key={proj}
              onClick={() => setSelectedProject(proj)}
              className={`w-full text-left px-3 py-2 rounded-md text-sm transition-colors ${
                selectedProject === proj
                  ? 'bg-gray-900 text-white'
                  : 'text-gray-700 hover:bg-gray-100'
              }`}
            >
              {proj}
            </button>
          ))}
        </div>
        <div className="px-4 py-3 border-t border-gray-200">
          <button
            onClick={fetchProjects}
            className="w-full px-3 py-1.5 text-xs text-gray-600 border border-gray-300 rounded-md hover:bg-gray-50"
          >
            Refresh
          </button>
        </div>
      </aside>

      {/* Project Detail */}
      <div className="flex-1 flex flex-col min-w-0">
        {!selectedProject && (
          <div className="flex items-center justify-center h-full text-gray-400 text-sm">
            Select a project from the sidebar
          </div>
        )}
        {selectedProject && (
          <>
            {/* Header */}
            <div className="px-6 py-4 border-b border-gray-200 bg-white shrink-0">
              <h2 className="text-lg font-semibold text-gray-900">{selectedProject}</h2>
              <div className="flex gap-4 mt-2 text-sm text-gray-600">
                <span>{runs.length} runs</span>
                <span className="text-green-600">{successCount} success</span>
                <span className="text-red-600">{errorCount} errors</span>
                <span>{feedback.length} feedback entries</span>
              </div>
            </div>

            {error && (
              <div className="px-6 py-3 bg-red-50 border-b border-red-200">
                <p className="text-sm text-red-700">{error}</p>
              </div>
            )}

            {/* Tabs */}
            <div className="px-6 pt-3 border-b border-gray-200 bg-white shrink-0">
              <div className="flex gap-4">
                <button
                  onClick={() => setTab('runs')}
                  className={`pb-2 text-sm border-b-2 transition-colors ${
                    tab === 'runs'
                      ? 'border-gray-900 text-gray-900 font-medium'
                      : 'border-transparent text-gray-500 hover:text-gray-700'
                  }`}
                >
                  Runs
                </button>
                <button
                  onClick={() => setTab('feedback')}
                  className={`pb-2 text-sm border-b-2 transition-colors ${
                    tab === 'feedback'
                      ? 'border-gray-900 text-gray-900 font-medium'
                      : 'border-transparent text-gray-500 hover:text-gray-700'
                  }`}
                >
                  Feedback
                </button>
              </div>
            </div>

            {/* Content */}
            <div className="flex-1 overflow-auto">
              {loading && (
                <div className="flex items-center justify-center h-32 text-gray-400 text-sm">Loading...</div>
              )}

              {!loading && tab === 'runs' && (
                <table className="w-full text-sm">
                  <thead className="bg-gray-50 sticky top-0">
                    <tr className="text-left text-xs font-medium text-gray-500 uppercase">
                      <th className="px-6 py-3">Name</th>
                      <th className="px-4 py-3">Type</th>
                      <th className="px-4 py-3">Status</th>
                      <th className="px-4 py-3">Latency</th>
                      <th className="px-4 py-3">Tokens</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-gray-100">
                    {runs.length === 0 && (
                      <tr>
                        <td colSpan={5} className="px-6 py-8 text-center text-gray-400">No runs in this project</td>
                      </tr>
                    )}
                    {runs.map((run) => (
                      <tr key={run.run_id} className="hover:bg-gray-50">
                        <td className="px-6 py-3 font-medium text-gray-900 truncate max-w-[200px]">{run.name}</td>
                        <td className="px-4 py-3">
                          <span className="px-2 py-0.5 bg-gray-100 text-gray-700 rounded text-xs">{run.run_type}</span>
                        </td>
                        <td className="px-4 py-3">
                          <span className={`px-2 py-0.5 rounded text-xs font-medium ${
                            run.status === 'success' ? 'bg-green-100 text-green-700' :
                            run.status === 'error' ? 'bg-red-100 text-red-700' :
                            'bg-blue-100 text-blue-700'
                          }`}>{run.status}</span>
                        </td>
                        <td className="px-4 py-3 text-gray-600">
                          {run.latency_ms !== null ? `${run.latency_ms}ms` : '-'}
                        </td>
                        <td className="px-4 py-3 text-gray-600">
                          {run.total_tokens !== null ? run.total_tokens : '-'}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}

              {!loading && tab === 'feedback' && (
                <table className="w-full text-sm">
                  <thead className="bg-gray-50 sticky top-0">
                    <tr className="text-left text-xs font-medium text-gray-500 uppercase">
                      <th className="px-6 py-3">Key</th>
                      <th className="px-4 py-3">Score</th>
                      <th className="px-4 py-3">Comment</th>
                      <th className="px-4 py-3">Run ID</th>
                      <th className="px-4 py-3">Created</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-gray-100">
                    {feedback.length === 0 && (
                      <tr>
                        <td colSpan={5} className="px-6 py-8 text-center text-gray-400">No feedback entries</td>
                      </tr>
                    )}
                    {feedback.map((fb) => (
                      <tr key={fb.id} className="hover:bg-gray-50">
                        <td className="px-6 py-3 font-medium text-gray-900">{fb.key}</td>
                        <td className="px-4 py-3">
                          <ScoreBadge score={fb.score} />
                        </td>
                        <td className="px-4 py-3 text-gray-600 truncate max-w-[200px]">{fb.comment || '-'}</td>
                        <td className="px-4 py-3 text-gray-500 font-mono text-xs truncate max-w-[120px]">{fb.run_id}</td>
                        <td className="px-4 py-3 text-gray-500 text-xs">
                          {new Date(fb.created_at).toLocaleString()}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function ScoreBadge({ score }: { score: number }) {
  const color = score >= 0.8
    ? 'bg-green-100 text-green-700'
    : score >= 0.5
      ? 'bg-yellow-100 text-yellow-700'
      : 'bg-red-100 text-red-700';
  return (
    <span className={`px-2 py-0.5 rounded text-xs font-medium ${color}`}>
      {score.toFixed(2)}
    </span>
  );
}
