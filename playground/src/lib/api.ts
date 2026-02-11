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
  type: 'node_start' | 'node_end' | 'complete' | 'graph_complete' | 'error';
  node_id?: string;
  step_number?: number;
  state?: Record<string, unknown>;
  output?: unknown;
  total_steps?: number;
  message?: string;
}

// --- Research ---

export interface ResearchSseEvent {
  type: 'progress' | 'complete' | 'error';
  message?: string;
  text?: string;
  interaction_id?: string;
}

// --- Helpers ---

const API_KEY_HEADERS: Record<Provider, string> = {
  gemini: 'X-Gemini-Key',
  claude: 'X-Anthropic-Key',
  openai: 'X-OpenAI-Key',
};

function getApiKeys(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem('ayas-api-keys') || '{}');
  } catch {
    return {};
  }
}

export function getApiKey(provider: Provider): string | undefined {
  const keys = getApiKeys();
  return keys[provider];
}

function buildHeaders(provider: Provider): Record<string, string> {
  const apiKey = getApiKey(provider);
  if (!apiKey) {
    throw new Error(`API key for ${provider} is not set. Please configure it in API Keys settings.`);
  }
  return {
    'Content-Type': 'application/json',
    [API_KEY_HEADERS[provider]]: apiKey,
  };
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
        // skip unparseable events
      }
    }
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
): Promise<void> {
  return streamSSE(
    '/api/graph/invoke-stream',
    { nodes, edges, channels, input },
    { 'Content-Type': 'application/json' },
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
}

export function pipelineInvokeStream(
  hypothesisCount: number,
  onEvent: (event: PipelineSseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const apiKey = getApiKey('gemini');
  if (!apiKey) {
    throw new Error('Gemini API key is required for Pipeline. Please configure it in API Keys settings.');
  }
  return streamSSE(
    '/api/pipeline/hypothesis',
    { hypothesis_count: hypothesisCount },
    {
      'Content-Type': 'application/json',
      'X-Gemini-Key': apiKey,
    },
    onEvent,
    signal,
  );
}

// --- Research API ---

export function researchInvokeStream(
  query: string,
  agent: string | undefined,
  previousInteractionId: string | undefined,
  onEvent: (event: ResearchSseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const apiKey = getApiKey('gemini');
  if (!apiKey) {
    throw new Error('Gemini API key is required for Research. Please configure it in API Keys settings.');
  }
  return streamSSE(
    '/api/research/invoke',
    {
      query,
      agent: agent || undefined,
      previous_interaction_id: previousInteractionId || undefined,
    },
    {
      'Content-Type': 'application/json',
      'X-Gemini-Key': apiKey,
    },
    onEvent,
    signal,
  );
}
