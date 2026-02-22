export type Provider = 'gemini' | 'claude' | 'openai';

export interface ChatMessage {
  type: 'user' | 'ai' | 'system';
  content: string;
}

// --- Chat ---

export interface ChatInvokeRequest {
  provider: Provider;
  model: string;
  messages: ChatMessage[];
  system_prompt?: string;
  temperature?: number;
  max_tokens?: number;
}

export interface ChatInvokeResponse {
  content: string;
  tokens_in: number;
  tokens_out: number;
}

// --- Agent ---

export interface AgentInvokeRequest {
  provider: Provider;
  model: string;
  tools: string[];
  messages: ChatMessage[];
  recursion_limit?: number;
}

export interface AgentSseEvent {
  type: 'step' | 'tool_call' | 'tool_result' | 'message' | 'done' | 'error';
  step_number?: number;
  node_name?: string;
  summary?: string;
  tool_name?: string;
  arguments?: Record<string, unknown>;
  result?: string;
  content?: string;
  total_steps?: number;
  message?: string;
}

// --- Graph ---

export interface GraphNodeDto {
  id: string;
  type: string;
  label?: string;
  config?: Record<string, unknown>;
}

export interface GraphEdgeDto {
  from: string;
  to: string;
  condition?: string;
  /** When true, edges sharing the same source node are executed in parallel (fan-out). */
  fan_out?: boolean;
  /** When true, this edge is only followed when the source node execution fails. */
  on_error?: boolean;
}

export interface GraphChannelDto {
  key: string;
  type: string;
  default?: unknown;
}

export interface GraphValidateResponse {
  valid: boolean;
  errors: string[];
}

export interface GraphSseEvent {
  type: 'node_start' | 'node_end' | 'complete' | 'graph_complete' | 'interrupted' | 'error';
  node_id?: string;
  step_number?: number;
  state?: Record<string, unknown>;
  output?: unknown;
  total_steps?: number;
  message?: string;
  // HITL fields
  session_id?: string;
  checkpoint_id?: string;
  interrupt_value?: unknown;
}

// --- Saved Graphs ---

export interface NodePosition {
  x: number;
  y: number;
}

export interface SavedGraphNode {
  id: string;
  type: string;
  label?: string;
  config?: Record<string, unknown>;
  position: NodePosition;
}

export interface GraphData {
  nodes: SavedGraphNode[];
  edges: GraphEdgeDto[];
  channels: GraphChannelDto[];
  node_counter: number;
}

export interface SavedGraph {
  id: string;
  name: string;
  description?: string;
  graph_data: GraphData;
  created_at: string;
  updated_at: string;
}

export interface GraphListItem {
  id: string;
  name: string;
  description?: string;
  created_at: string;
  updated_at: string;
}

// --- Research ---

export interface ResearchSseEvent {
  type: 'progress' | 'complete' | 'error';
  message?: string;
  text?: string;
  interaction_id?: string;
}

// --- Env keys cache ---

export interface EnvKeys {
  gemini: boolean;
  claude: boolean;
  openai: boolean;
}

let envKeysCache: EnvKeys | null = null;

export async function fetchEnvKeys(): Promise<EnvKeys> {
  if (envKeysCache) return envKeysCache;
  try {
    const res = await fetch('/api/env-keys', { signal: AbortSignal.timeout(3000) });
    if (res.ok) {
      envKeysCache = await res.json();
      return envKeysCache!;
    }
  } catch { /* ignore */ }
  return { gemini: false, claude: false, openai: false };
}

export function getCachedEnvKeys(): EnvKeys | null {
  return envKeysCache;
}

// --- Helpers ---

function buildHeaders(_provider: Provider): Record<string, string> {
  // API keys are provided via backend environment variables
  return { 'Content-Type': 'application/json' };
}

async function handleResponse<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text();
    if (res.status === 400 && text.toLowerCase().includes('key')) {
      throw new Error('API key is missing or invalid. Please check your API Keys settings.');
    }
    if (res.status === 429) {
      throw new Error('Rate limited. Please wait a moment and try again.');
    }
    throw new Error(`API error (${res.status}): ${text}`);
  }
  return res.json();
}

// SSE parser: reads server-sent events and calls onEvent for each parsed JSON event
export async function streamSSE<T>(
  url: string,
  body: unknown,
  headers: Record<string, string>,
  onEvent: (event: T) => void,
  signal?: AbortSignal,
): Promise<void> {
  const res = await fetch(url, {
    method: 'POST',
    headers,
    body: JSON.stringify(body),
    signal,
  });

  if (!res.ok) {
    const text = await res.text();
    throw new Error(`API error (${res.status}): ${text}`);
  }

  const reader = res.body?.getReader();
  if (!reader) throw new Error('No response body');

  const decoder = new TextDecoder();
  let buffer = '';

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() || '';

      for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed.startsWith('data:')) continue;
        const data = trimmed.slice(5).trim();
        if (data === '[DONE]') return;
        try {
          onEvent(JSON.parse(data) as T);
        } catch {
          // skip unparseable events (including keepalive comments)
        }
      }
    }
  } catch (err) {
    // Re-throw AbortError as-is; wrap network errors with more context
    if (err instanceof DOMException && err.name === 'AbortError') throw err;
    const msg = err instanceof Error ? err.message : String(err);
    throw new Error(
      `SSE connection lost: ${msg}. The server may still be processing. ` +
      `If this persists, try running the pipeline again.`
    );
  }
}

// --- Chat API ---

export async function chatInvoke(req: ChatInvokeRequest): Promise<ChatInvokeResponse> {
  const headers = buildHeaders(req.provider);
  const res = await fetch('/api/chat/invoke', {
    method: 'POST',
    headers,
    body: JSON.stringify(req),
  });
  return handleResponse<ChatInvokeResponse>(res);
}

// --- Agent API ---

export function agentInvokeStream(
  req: AgentInvokeRequest,
  onEvent: (event: AgentSseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const headers = buildHeaders(req.provider);
  return streamSSE('/api/agent/invoke', req, headers, onEvent, signal);
}

// --- Graph API ---

export async function graphValidate(
  nodes: GraphNodeDto[],
  edges: GraphEdgeDto[],
  channels: GraphChannelDto[],
): Promise<GraphValidateResponse> {
  const res = await fetch('/api/graph/validate', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ nodes, edges, channels }),
  });
  return handleResponse<GraphValidateResponse>(res);
}

export function graphInvokeStream(
  nodes: GraphNodeDto[],
  edges: GraphEdgeDto[],
  channels: GraphChannelDto[],
  input: unknown,
  onEvent: (event: GraphSseEvent) => void,
  signal?: AbortSignal,
  recursionLimit?: number,
): Promise<void> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  const body: Record<string, unknown> = { nodes, edges, channels, input };
  if (recursionLimit !== undefined) body.recursion_limit = recursionLimit;
  return streamSSE(
    '/api/graph/invoke-stream',
    body,
    headers,
    onEvent,
    signal,
  );
}

export async function graphGenerate(
  prompt: string,
  provider: Provider,
  model: string,
): Promise<{ nodes: GraphNodeDto[]; edges: GraphEdgeDto[]; channels: GraphChannelDto[] }> {
  const headers = buildHeaders(provider);
  const res = await fetch('/api/graph/generate', {
    method: 'POST',
    headers,
    body: JSON.stringify({ prompt, provider, model }),
  });
  return handleResponse(res);
}

// --- HITL API ---

export function graphExecuteResumable(
  threadId: string,
  nodes: GraphNodeDto[],
  edges: GraphEdgeDto[],
  channels: GraphChannelDto[],
  input: unknown,
  onEvent: (event: GraphSseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  return streamSSE(
    '/api/graph/execute-resumable',
    { thread_id: threadId, nodes, edges, channels, input },
    headers,
    onEvent,
    signal,
  );
}

export function graphResume(
  sessionId: string,
  resumeValue: unknown,
  onEvent: (event: GraphSseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  return streamSSE(
    '/api/graph/resume',
    { session_id: sessionId, resume_value: resumeValue },
    headers,
    onEvent,
    signal,
  );
}

// --- Saved Graphs API ---

export async function saveGraph(
  name: string,
  graphData: GraphData,
  description?: string,
): Promise<{ id: string; name: string; created_at: string }> {
  const res = await fetch('/api/graphs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, description, graph_data: graphData }),
  });
  return handleResponse(res);
}

export async function listGraphs(): Promise<GraphListItem[]> {
  const res = await fetch('/api/graphs');
  return handleResponse<GraphListItem[]>(res);
}

export async function getGraph(id: string): Promise<SavedGraph> {
  const res = await fetch(`/api/graphs/${encodeURIComponent(id)}`);
  return handleResponse<SavedGraph>(res);
}

export async function updateGraph(
  id: string,
  graphData: GraphData,
  name?: string,
): Promise<SavedGraph> {
  const body: Record<string, unknown> = { graph_data: graphData };
  if (name !== undefined) body.name = name;
  const res = await fetch(`/api/graphs/${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  return handleResponse<SavedGraph>(res);
}

export async function deleteGraph(id: string): Promise<{ deleted: boolean }> {
  const res = await fetch(`/api/graphs/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
  return handleResponse(res);
}

// --- Research API ---

// --- Smith Runs API ---

export interface RunSummary {
  run_id: string;
  name: string;
  run_type: string;
  status: string;
  project: string;
  start_time: string;
  latency_ms: number | null;
  total_tokens: number | null;
}

export interface RunDto {
  run_id: string;
  parent_run_id: string | null;
  trace_id: string | null;
  name: string;
  run_type: string;
  project: string;
  start_time: string;
  end_time: string | null;
  status: string;
  input: string;
  output: string | null;
  error: string | null;
  tags: string[];
  metadata: string;
  input_tokens: number | null;
  output_tokens: number | null;
  total_tokens: number | null;
  latency_ms: number | null;
}

export interface RunFilterRequest {
  project?: string;
  run_type?: string;
  status?: string;
  name?: string;
  start_after?: string;
  start_before?: string;
  trace_id?: string;
  limit?: number;
  offset?: number;
}

export interface StatsResponse {
  tokens: {
    total_input_tokens: number;
    total_output_tokens: number;
    total_tokens: number;
    run_count: number;
  };
  latency: {
    p50: number;
    p90: number;
    p95: number;
    p99: number;
  };
}

export interface FeedbackDto {
  id: string;
  run_id: string;
  key: string;
  score: number;
  comment: string | null;
  created_at: string;
}

export async function queryRuns(filter: RunFilterRequest): Promise<RunSummary[]> {
  const res = await fetch('/api/runs/query', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(filter),
  });
  return handleResponse<RunSummary[]>(res);
}

export async function getRun(runId: string, project: string): Promise<RunDto> {
  const res = await fetch(`/api/runs/${runId}?project=${encodeURIComponent(project)}`);
  return handleResponse<RunDto>(res);
}

export async function getTrace(traceId: string, project: string): Promise<RunDto[]> {
  const res = await fetch(`/api/runs/trace/${traceId}?project=${encodeURIComponent(project)}`);
  return handleResponse<RunDto[]>(res);
}

export async function getStats(project?: string): Promise<StatsResponse> {
  const params = project ? `?project=${encodeURIComponent(project)}` : '';
  const res = await fetch(`/api/runs/stats${params}`);
  return handleResponse<StatsResponse>(res);
}

export async function submitFeedback(
  runId: string,
  key: string,
  score: number,
  comment?: string,
): Promise<{ id: string; run_id: string; key: string; score: number }> {
  const res = await fetch('/api/feedback', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ run_id: runId, key, score, comment }),
  });
  return handleResponse(res);
}

export async function queryFeedback(
  runId?: string,
  key?: string,
): Promise<FeedbackDto[]> {
  const res = await fetch('/api/feedback/query', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ run_id: runId, key }),
  });
  return handleResponse<FeedbackDto[]>(res);
}

// --- Checkpoint API (for time-travel) ---

export interface CheckpointSession {
  session_id: string;
  thread_id: string;
  checkpoint_id: string;
  interrupt_value: unknown;
  graph_definition: unknown;
  created_at: string;
}

export async function listCheckpointSessions(): Promise<CheckpointSession[]> {
  const res = await fetch('/api/graph/sessions');
  return handleResponse<CheckpointSession[]>(res);
}

// --- Pipeline API ---

export interface PipelineSseEvent {
  type: 'step_start' | 'step_complete' | 'hypothesis' | 'step3_start' | 'step3_complete' | 'step3_error' | 'complete' | 'error';
  step?: number;
  description?: string;
  summary?: string;
  index?: number;
  title?: string;
  score?: number;
  text?: string;
  step1_text?: string;
  hypotheses?: unknown;
  message?: string;
  physical_contradiction?: string;
  cap_id_fingerprint?: string;
  verdict_tag?: string;
  verdict_reason?: string;
}

export interface PipelineInvokeParams {
  mode: 'llm' | 'manual';
  hypothesis_count: number;
  needs?: string;
  seeds?: string;
  hypotheses?: { title: string }[];
}

export function pipelineInvokeStream(
  params: PipelineInvokeParams,
  onEvent: (event: PipelineSseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  return streamSSE('/api/pipeline/hypothesis', params, headers, onEvent, signal);
}

// --- Research API ---

export function researchInvokeStream(
  query: string,
  agent: string | undefined,
  previousInteractionId: string | undefined,
  onEvent: (event: ResearchSseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  return streamSSE(
    '/api/research/invoke',
    {
      query,
      agent: agent || undefined,
      previous_interaction_id: previousInteractionId || undefined,
    },
    headers,
    onEvent,
    signal,
  );
}
