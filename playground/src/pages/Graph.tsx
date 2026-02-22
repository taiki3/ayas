import { useState, useCallback, useRef, useEffect } from 'react';
import {
  ReactFlow,
  addEdge,
  useNodesState,
  useEdgesState,
  Controls,
  Background,
  BackgroundVariant,
  type Node,
  type Edge,
  type OnConnect,
  type NodeTypes,
  Handle,
  Position,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {
  graphValidate,
  graphInvokeStream,
  graphGenerate,
  graphExecuteResumable,
  graphResume,
  saveGraph,
  listGraphs,
  getGraph,
  updateGraph,
  deleteGraph,
  type Provider,
  type GraphNodeDto,
  type GraphEdgeDto,
  type GraphChannelDto,
  type GraphSseEvent,
  type GraphData,
  type GraphListItem,
} from '../lib/api';
import { GRAPH_TEMPLATES, type GraphTemplate } from '../data/graph-templates';
import dagre from '@dagrejs/dagre';

// --- Custom Node Components ---

function StartNode() {
  return (
    <div className="px-4 py-2 bg-green-100 border-2 border-green-500 rounded-lg text-sm font-medium text-green-800">
      START
      <Handle type="source" position={Position.Bottom} className="!bg-green-500" />
    </div>
  );
}

function EndNode() {
  return (
    <div className="px-4 py-2 bg-red-100 border-2 border-red-500 rounded-lg text-sm font-medium text-red-800">
      <Handle type="target" position={Position.Top} className="!bg-red-500" />
      END
    </div>
  );
}

function LlmNode({ data }: { data: { label: string; active?: boolean } }) {
  return (
    <div className={`px-4 py-2 border-2 rounded-lg text-sm min-w-[120px] ${
      data.active ? 'border-blue-500 bg-blue-50 animate-pulse' : 'border-border bg-card'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-muted-foreground" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">💬</span>
        <span className="font-medium text-foreground">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-muted-foreground" />
    </div>
  );
}

function ConditionalNode({ data }: { data: { label: string; active?: boolean } }) {
  return (
    <div className={`px-4 py-2 border-2 rounded-lg text-sm min-w-[120px] rotate-0 ${
      data.active ? 'border-yellow-500 bg-yellow-50 animate-pulse' : 'border-yellow-300 bg-yellow-50'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-yellow-500" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">◇</span>
        <span className="font-medium text-foreground">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-yellow-500" />
    </div>
  );
}

function TransformNode({ data }: { data: { label: string; active?: boolean } }) {
  return (
    <div className={`px-4 py-2 border-2 rounded-lg text-sm min-w-[120px] ${
      data.active ? 'border-gray-500 bg-muted animate-pulse' : 'border-border bg-surface'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-gray-500" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">⚙</span>
        <span className="font-medium text-foreground">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-gray-500" />
    </div>
  );
}

function InterruptNode({ data }: { data: { label: string; active?: boolean } }) {
  return (
    <div className={`px-4 py-2 border-2 rounded-lg text-sm min-w-[120px] ${
      data.active ? 'border-amber-500 bg-amber-50 animate-pulse' : 'border-amber-300 bg-amber-50'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-amber-500" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">&#x23F8;</span>
        <span className="font-medium text-foreground">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-amber-500" />
    </div>
  );
}

function AgentNode({ data }: { data: { label: string; active?: boolean } }) {
  return (
    <div className={`px-4 py-2 border-2 rounded-lg text-sm min-w-[120px] ${
      data.active ? 'border-emerald-500 bg-emerald-50 animate-pulse' : 'border-emerald-300 bg-emerald-50'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-emerald-500" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">&#x1F916;</span>
        <span className="font-medium text-foreground">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-emerald-500" />
    </div>
  );
}

function DeepResearchNode({ data }: { data: { label: string; active?: boolean } }) {
  return (
    <div className={`px-4 py-2 border-2 rounded-lg text-sm min-w-[120px] ${
      data.active ? 'border-purple-500 bg-purple-50 animate-pulse' : 'border-purple-300 bg-purple-50'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-purple-500" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">🔬</span>
        <span className="font-medium text-foreground">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-purple-500" />
    </div>
  );
}

const nodeTypes: NodeTypes = {
  start: StartNode,
  end: EndNode,
  llm: LlmNode,
  agent: AgentNode,
  interrupt: InterruptNode,
  conditional: ConditionalNode,
  transform: TransformNode,
  passthrough: TransformNode,
  deep_research: DeepResearchNode,
};

// --- Helper: get/update config for a node ---

type NodeConfig = Record<string, unknown>;

function getNodeConfig(node: Node): NodeConfig {
  return ((node.data as Record<string, unknown>)?.config as NodeConfig) || {};
}

// --- Graph Page ---

const INITIAL_NODES: Node[] = [
  { id: 'start', type: 'start', position: { x: 250, y: 0 }, data: {} },
  { id: 'end', type: 'end', position: { x: 250, y: 300 }, data: {} },
];
const INITIAL_EDGES: Edge[] = [];

type ChannelEntry = { key: string; type: string; default?: string };

const MODEL_OPTIONS: Record<string, { id: string; label: string }[]> = {
  gemini: [
    { id: 'gemini-2.5-flash', label: 'Gemini 2.5 Flash' },
    { id: 'gemini-2.5-flash-lite', label: 'Gemini 2.5 Flash-Lite' },
    { id: 'gemini-2.5-pro', label: 'Gemini 2.5 Pro' },
    { id: 'gemini-3-flash-preview', label: 'Gemini 3 Flash (Preview)' },
    { id: 'gemini-3-pro-preview', label: 'Gemini 3 Pro (Preview)' },
    { id: 'deep-research-pro-preview-12-2025', label: 'Deep Research Pro' },
  ],
  claude: [
    { id: 'claude-opus-4-6', label: 'Claude Opus 4.6' },
    { id: 'claude-opus-4-5-20251101', label: 'Claude Opus 4.5' },
    { id: 'claude-sonnet-4-5-20250929', label: 'Claude Sonnet 4.5' },
    { id: 'claude-haiku-4-5-20251001', label: 'Claude Haiku 4.5' },
  ],
  openai: [
    { id: 'gpt-5.2', label: 'GPT-5.2' },
    { id: 'gpt-5.2-pro', label: 'GPT-5.2 Pro' },
  ],
};

const AVAILABLE_TOOLS = ['calculator', 'datetime', 'web_search'];

const DEFAULT_LLM_CONFIG: NodeConfig = {
  provider: 'gemini',
  model: 'gemini-2.5-flash',
  prompt: '',
  temperature: 0.7,
  input_channel: 'value',
  output_channel: 'value',
  tools: [] as string[],
  max_tool_iterations: 5,
};

const DEFAULT_AGENT_CONFIG: NodeConfig = {
  provider: 'gemini',
  model: 'gemini-2.5-flash',
  system_prompt: '',
  tools: ['calculator', 'datetime'] as string[],
  recursion_limit: 25,
  input_channel: 'value',
  output_channel: 'value',
};

const DEFAULT_TRANSFORM_CONFIG: NodeConfig = {
  expression: '',
  output_channel: 'value',
};

const DEEP_RESEARCH_AGENT_OPTIONS = [
  { id: 'deep-research-pro-preview-12-2025', label: 'Deep Research Pro' },
];

const DEFAULT_DEEP_RESEARCH_CONFIG: NodeConfig = {
  agent: 'deep-research-pro-preview-12-2025',
  prompt: '',
  input_channel: 'value',
  output_channel: 'value',
  attachments_channel: '',
  file_search_store_names: '',
};

export default function Graph() {
  const [nodes, setNodes, onNodesChange] = useNodesState(INITIAL_NODES);
  const [edges, setEdges, onEdgesChange] = useEdgesState(INITIAL_EDGES);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);
  const [channels, setChannels] = useState<ChannelEntry[]>([
    { key: 'value', type: 'LastValue', default: '' },
  ]);
  const [toast, setToast] = useState<{ type: 'success' | 'error'; message: string } | null>(null);
  const [runModal, setRunModal] = useState(false);
  const [runInput, setRunInput] = useState('{"value": "hello"}');
  const [runOutput, setRunOutput] = useState<string | null>(null);
  const [runSteps, setRunSteps] = useState<GraphSseEvent[]>([]);
  const [running, setRunning] = useState(false);
  const [recursionLimit, setRecursionLimit] = useState(25);
  const [hitlMode, setHitlMode] = useState(false);
  const [hitlSessionId, setHitlSessionId] = useState<string | null>(null);
  const [hitlInterruptValue, setHitlInterruptValue] = useState<string>('');
  const [hitlResumeInput, setHitlResumeInput] = useState('');
  const [stateHistory, setStateHistory] = useState<Map<string, { step: number; state: Record<string, unknown> }[]>>(new Map());
  const [inspectorTab, setInspectorTab] = useState<'config' | 'state'>('config');

  // Undo/Redo
  type GraphSnapshot = { nodes: Node[]; edges: Edge[]; channels: ChannelEntry[] };
  const undoStack = useRef<GraphSnapshot[]>([]);
  const redoStack = useRef<GraphSnapshot[]>([]);
  const skipSnapshot = useRef(false);

  const pushUndo = useCallback(() => {
    if (skipSnapshot.current) return;
    undoStack.current.push({
      nodes: JSON.parse(JSON.stringify(nodes)),
      edges: JSON.parse(JSON.stringify(edges)),
      channels: JSON.parse(JSON.stringify(channels)),
    });
    redoStack.current = [];
    if (undoStack.current.length > 50) undoStack.current.shift();
  }, [nodes, edges, channels]);

  const handleUndo = useCallback(() => {
    if (undoStack.current.length === 0) return;
    redoStack.current.push({
      nodes: JSON.parse(JSON.stringify(nodes)),
      edges: JSON.parse(JSON.stringify(edges)),
      channels: JSON.parse(JSON.stringify(channels)),
    });
    const snap = undoStack.current.pop()!;
    skipSnapshot.current = true;
    setNodes(snap.nodes);
    setEdges(snap.edges);
    setChannels(snap.channels);
    setTimeout(() => { skipSnapshot.current = false; }, 50);
  }, [nodes, edges, channels, setNodes, setEdges]);

  const handleRedo = useCallback(() => {
    if (redoStack.current.length === 0) return;
    undoStack.current.push({
      nodes: JSON.parse(JSON.stringify(nodes)),
      edges: JSON.parse(JSON.stringify(edges)),
      channels: JSON.parse(JSON.stringify(channels)),
    });
    const snap = redoStack.current.pop()!;
    skipSnapshot.current = true;
    setNodes(snap.nodes);
    setEdges(snap.edges);
    setChannels(snap.channels);
    setTimeout(() => { skipSnapshot.current = false; }, 50);
  }, [nodes, edges, channels, setNodes, setEdges]);

  // Keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'z' && !e.shiftKey) {
        e.preventDefault();
        handleUndo();
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'z' && e.shiftKey) {
        e.preventDefault();
        handleRedo();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [handleUndo, handleRedo]);
  const [addNodeModal, setAddNodeModal] = useState(false);
  const [newNodeType, setNewNodeType] = useState('llm');
  const [newNodeLabel, setNewNodeLabel] = useState('');
  const nodeCounter = useRef(1);
  // Save/Load state
  const [currentGraphId, setCurrentGraphId] = useState<string | null>(null);
  const [currentGraphName, setCurrentGraphName] = useState<string | null>(null);
  const [saveModal, setSaveModal] = useState(false);
  const [loadModal, setLoadModal] = useState(false);
  const [savedGraphs, setSavedGraphs] = useState<GraphListItem[]>([]);
  const [saveName, setSaveName] = useState('');
  // Templates & AI Generate & Import
  const [templateModal, setTemplateModal] = useState(false);
  const [generateModal, setGenerateModal] = useState(false);
  const [generatePrompt, setGeneratePrompt] = useState('');
  const [generateProvider, setGenerateProvider] = useState<Provider>('gemini');
  const [generateModel, setGenerateModel] = useState('gemini-2.5-flash');
  const [generating, setGenerating] = useState(false);
  const importInputRef = useRef<HTMLInputElement>(null);

  const onConnect: OnConnect = useCallback(
    (params) => {
      pushUndo();
      setEdges((eds) => addEdge(params, eds));
    },
    [setEdges, pushUndo],
  );

  const selectedNode = nodes.find((n) => n.id === selectedNodeId);
  const selectedEdge = edges.find((e) => e.id === selectedEdgeId);

  const showToast = (type: 'success' | 'error', message: string) => {
    setToast({ type, message });
    setTimeout(() => setToast(null), 4000);
  };

  const updateNodeConfig = (nodeId: string, key: string, value: unknown) => {
    setNodes((nds) =>
      nds.map((n) => {
        if (n.id !== nodeId) return n;
        const prevConfig = getNodeConfig(n);
        return { ...n, data: { ...n.data, config: { ...prevConfig, [key]: value } } };
      }),
    );
  };

  // Convert ReactFlow nodes/edges to API DTOs
  const toApiNodes = (): GraphNodeDto[] =>
    nodes
      .filter((n) => n.type !== 'start' && n.type !== 'end')
      .map((n) => {
        const config = (n.data as Record<string, unknown>).config as Record<string, unknown> | undefined;
        // Convert file_search_store_names from newline-separated string to array
        if (config && typeof config.file_search_store_names === 'string') {
          const names = (config.file_search_store_names as string)
            .split('\n')
            .map((s) => s.trim())
            .filter((s) => s.length > 0);
          config.file_search_store_names = names.length > 0 ? names : undefined;
        }
        return {
          id: n.id,
          type: n.type || 'passthrough',
          label: (n.data as Record<string, string>).label || n.id,
          config,
        };
      });

  const toApiEdges = (): GraphEdgeDto[] =>
    edges.map((e) => ({
      from: e.source,
      to: e.target,
      condition: (e.data as Record<string, unknown>)?.condition as string | undefined,
      fan_out: !!(e.data as Record<string, unknown>)?.fan_out,
      on_error: !!(e.data as Record<string, unknown>)?.on_error,
    }));

  const toApiChannels = (): GraphChannelDto[] =>
    channels.map((c) => ({
      key: c.key,
      type: c.type,
      default: c.default || undefined,
    }));

  const handleValidate = async () => {
    try {
      const result = await graphValidate(toApiNodes(), toApiEdges(), toApiChannels());
      if (result.valid) {
        showToast('success', 'Graph is valid ✓');
      } else {
        showToast('error', result.errors.join('; '));
      }
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Validation failed');
    }
  };

  const handleSseEvent = (event: GraphSseEvent) => {
    setRunSteps((prev) => [...prev, event]);

    if (event.type === 'node_start' && event.node_id) {
      setNodes((nds) =>
        nds.map((n) =>
          n.id === event.node_id
            ? { ...n, data: { ...n.data, active: true } }
            : n,
        ),
      );
    }
    if (event.type === 'node_end' && event.node_id) {
      setNodes((nds) =>
        nds.map((n) =>
          n.id === event.node_id
            ? { ...n, data: { ...n.data, active: false } }
            : n,
        ),
      );
      // Capture state for inspector
      if (event.state) {
        setStateHistory((prev) => {
          const next = new Map(prev);
          const existing = next.get(event.node_id!) || [];
          next.set(event.node_id!, [...existing, {
            step: event.step_number ?? existing.length,
            state: event.state as Record<string, unknown>,
          }]);
          return next;
        });
      }
    }
    if (event.type === 'complete' || event.type === 'graph_complete') {
      setRunOutput(JSON.stringify(event.output, null, 2));
      setHitlSessionId(null);
      setHitlInterruptValue('');
    }
    if (event.type === 'interrupted') {
      setHitlSessionId(event.session_id || null);
      setHitlInterruptValue(
        typeof event.interrupt_value === 'string'
          ? event.interrupt_value
          : JSON.stringify(event.interrupt_value, null, 2),
      );
      setHitlResumeInput('');
      setRunning(false);
    }
    if (event.type === 'error') {
      showToast('error', event.message || 'Execution error');
    }
  };

  const handleRun = async () => {
    setRunning(true);
    setRunOutput(null);
    setRunSteps([]);
    setStateHistory(new Map());
    setHitlSessionId(null);
    setHitlInterruptValue('');

    let inputJson: unknown;
    try {
      inputJson = JSON.parse(runInput);
    } catch {
      showToast('error', 'Invalid JSON input');
      setRunning(false);
      return;
    }

    try {
      if (hitlMode) {
        const threadId = `thread-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
        await graphExecuteResumable(
          threadId,
          toApiNodes(),
          toApiEdges(),
          toApiChannels(),
          inputJson,
          handleSseEvent,
        );
      } else {
        await graphInvokeStream(
          toApiNodes(),
          toApiEdges(),
          toApiChannels(),
          inputJson,
          handleSseEvent,
          undefined,
          recursionLimit,
        );
      }
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Execution failed');
    } finally {
      if (!hitlSessionId) setRunning(false);
      // Clear active highlights
      setNodes((nds) => nds.map((n) => ({ ...n, data: { ...n.data, active: false } })));
    }
  };

  const handleResume = async () => {
    if (!hitlSessionId) return;
    setRunning(true);

    let resumeValue: unknown;
    try {
      resumeValue = JSON.parse(hitlResumeInput);
    } catch {
      // If not valid JSON, treat as plain string
      resumeValue = hitlResumeInput;
    }

    try {
      await graphResume(hitlSessionId, resumeValue, handleSseEvent);
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Resume failed');
    } finally {
      if (!hitlSessionId) setRunning(false);
      setNodes((nds) => nds.map((n) => ({ ...n, data: { ...n.data, active: false } })));
    }
  };

  const handleAddNode = () => {
    if (!newNodeLabel.trim()) return;
    pushUndo();
    const id = `${newNodeType}_${nodeCounter.current++}`;
    const defaultConfig =
      newNodeType === 'llm' ? { ...DEFAULT_LLM_CONFIG } :
      newNodeType === 'agent' ? { ...DEFAULT_AGENT_CONFIG } :
      newNodeType === 'interrupt' ? { value: 'Please review and approve' } :
      newNodeType === 'deep_research' ? { ...DEFAULT_DEEP_RESEARCH_CONFIG } :
      newNodeType === 'transform' ? { ...DEFAULT_TRANSFORM_CONFIG } :
      undefined;
    const newNode: Node = {
      id,
      type: newNodeType,
      position: { x: 200 + Math.random() * 100, y: 100 + nodes.length * 60 },
      data: { label: newNodeLabel, ...(defaultConfig ? { config: defaultConfig } : {}) },
    };
    setNodes((nds) => [...nds, newNode]);
    setNewNodeLabel('');
    setAddNodeModal(false);
  };

  // --- Save/Load helpers ---

  const toGraphData = (): GraphData => ({
    nodes: nodes.map((n) => ({
      id: n.id,
      type: n.type || 'passthrough',
      label: (n.data as Record<string, string>).label || undefined,
      config: (n.data as Record<string, unknown>).config as Record<string, unknown> | undefined,
      position: { x: n.position.x, y: n.position.y },
    })),
    edges: edges.map((e) => ({
      from: e.source,
      to: e.target,
      condition: (e.data as Record<string, unknown>)?.condition as string | undefined,
      fan_out: !!(e.data as Record<string, unknown>)?.fan_out,
      on_error: !!(e.data as Record<string, unknown>)?.on_error,
    })),
    channels: channels.map((c) => ({
      key: c.key,
      type: c.type,
      default: c.default || undefined,
    })),
    node_counter: nodeCounter.current,
  });

  const loadGraphData = (data: GraphData) => {
    const posMap = new Map(data.nodes.map((n) => [n.id, n.position]));
    const newNodes: Node[] = data.nodes.map((n) => ({
      id: n.id,
      type: n.type,
      position: { x: n.position.x, y: n.position.y },
      data: {
        ...(n.label ? { label: n.label } : {}),
        ...(n.config ? { config: n.config } : {}),
      },
    }));
    const newEdges: Edge[] = data.edges.map((e, i) => {
      const srcPos = posMap.get(e.from);
      const tgtPos = posMap.get(e.to);
      const isBackEdge = srcPos && tgtPos && tgtPos.y < srcPos.y;
      return {
        id: `e-${e.from}-${e.to}-${i}`,
        source: e.from,
        target: e.to,
        ...(e.on_error ? {
          style: { strokeWidth: 2, strokeDasharray: '5 3', stroke: '#ef4444' },
          label: 'on error',
          labelStyle: { fill: '#ef4444', fontSize: 10, fontWeight: 600 },
        } : e.fan_out ? {
          style: { strokeWidth: 2, strokeDasharray: '6 3', stroke: '#f97316' },
          label: 'parallel',
          labelStyle: { fill: '#f97316', fontSize: 10, fontWeight: 600 },
        } : isBackEdge ? {
          style: { strokeWidth: 2, strokeDasharray: '4 4', stroke: '#8b5cf6' },
          animated: true,
          label: 'loop',
          labelStyle: { fill: '#8b5cf6', fontSize: 10, fontWeight: 600 },
        } : {}),
        ...(e.condition && !e.fan_out && !e.on_error && !isBackEdge ? { label: e.condition } : {}),
        data: { fan_out: !!e.fan_out, on_error: !!e.on_error, condition: e.condition || undefined },
      };
    });
    const newChannels: ChannelEntry[] = data.channels.map((c) => ({
      key: c.key,
      type: c.type,
      default: c.default != null ? String(c.default) : undefined,
    }));
    setNodes(newNodes);
    setEdges(newEdges);
    setChannels(newChannels.length > 0 ? newChannels : [{ key: 'value', type: 'LastValue', default: '' }]);
    nodeCounter.current = data.node_counter || 1;
  };

  const handleSave = async () => {
    const name = saveName.trim();
    if (!name) return;
    try {
      const data = toGraphData();
      if (currentGraphId) {
        await updateGraph(currentGraphId, data, name);
        setCurrentGraphName(name);
        showToast('success', `Saved "${name}"`);
      } else {
        const res = await saveGraph(name, data);
        setCurrentGraphId(res.id);
        setCurrentGraphName(name);
        showToast('success', `Saved "${name}"`);
      }
      setSaveModal(false);
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Save failed');
    }
  };

  const handleLoad = async (id: string) => {
    try {
      const graph = await getGraph(id);
      loadGraphData(graph.graph_data);
      setCurrentGraphId(graph.id);
      setCurrentGraphName(graph.name);
      setLoadModal(false);
      showToast('success', `Loaded "${graph.name}"`);
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Load failed');
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteGraph(id);
      setSavedGraphs((prev) => prev.filter((g) => g.id !== id));
      if (currentGraphId === id) {
        setCurrentGraphId(null);
        setCurrentGraphName(null);
      }
      showToast('success', 'Graph deleted');
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Delete failed');
    }
  };

  const openLoadModal = async () => {
    try {
      const graphs = await listGraphs();
      setSavedGraphs(graphs);
    } catch {
      setSavedGraphs([]);
    }
    setLoadModal(true);
  };

  const openSaveModal = () => {
    setSaveName(currentGraphName || '');
    setSaveModal(true);
  };

  // --- Template ---
  const handleApplyTemplate = (template: GraphTemplate) => {
    pushUndo();
    loadGraphData(template.data);
    setCurrentGraphId(null);
    setCurrentGraphName(null);
    setTemplateModal(false);
    showToast('success', `Applied template: ${template.name}`);
  };

  // --- AI Generate ---
  const handleGenerate = async () => {
    if (!generatePrompt.trim()) return;
    setGenerating(true);
    try {
      const result = await graphGenerate(generatePrompt, generateProvider, generateModel);
      pushUndo();
      // Build GraphData with auto-layout positions
      const graphData: GraphData = {
        nodes: [
          { id: 'start', type: 'start', position: { x: 250, y: 0 } },
          ...result.nodes.map((n, i) => ({
            ...n,
            position: { x: 200, y: 120 + i * 140 },
          })),
          { id: 'end', type: 'end', position: { x: 250, y: 120 + result.nodes.length * 140 } },
        ],
        edges: result.edges,
        channels: result.channels,
        node_counter: result.nodes.length + 1,
      };
      loadGraphData(graphData);
      setCurrentGraphId(null);
      setCurrentGraphName(null);
      setGenerateModal(false);
      showToast('success', 'Graph generated from AI');
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Generation failed');
    } finally {
      setGenerating(false);
    }
  };

  // --- Auto-layout (dagre) ---
  const handleAutoLayout = () => {
    pushUndo();
    const g = new dagre.graphlib.Graph();
    g.setDefaultEdgeLabel(() => ({}));
    g.setGraph({ rankdir: 'TB', nodesep: 60, ranksep: 100 });

    for (const node of nodes) {
      // Use measured dimensions from ReactFlow, fall back to estimates
      const w = node.measured?.width ?? 150;
      const h = node.measured?.height ?? 50;
      g.setNode(node.id, { width: w, height: h });
    }
    for (const edge of edges) {
      g.setEdge(edge.source, edge.target);
    }

    dagre.layout(g);

    setNodes((nds) =>
      nds.map((node) => {
        const dagreNode = g.node(node.id);
        if (!dagreNode) return node;
        const w = node.measured?.width ?? 150;
        const h = node.measured?.height ?? 50;
        // dagre returns center coordinates; convert to top-left for ReactFlow
        return { ...node, position: { x: dagreNode.x - w / 2, y: dagreNode.y - h / 2 } };
      }),
    );
    showToast('success', 'Auto-layout applied');
  };

  // --- Export/Import ---
  const handleExport = () => {
    const data = toGraphData();
    const json = JSON.stringify(data, null, 2);
    const blob = new Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${currentGraphName || 'graph'}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleImport = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => {
      try {
        const data = JSON.parse(ev.target?.result as string) as GraphData;
        if (!data.nodes || !data.edges) {
          showToast('error', 'Invalid graph JSON: missing nodes or edges');
          return;
        }
        pushUndo();
        loadGraphData(data);
        setCurrentGraphId(null);
        setCurrentGraphName(file.name.replace(/\.json$/, ''));
        showToast('success', `Imported: ${file.name}`);
      } catch {
        showToast('error', 'Invalid JSON file');
      }
    };
    reader.readAsText(file);
    // Reset input so the same file can be re-imported
    e.target.value = '';
  };

  // --- File attachment helpers ---
  type Attachment = { data: string; media_type: string; name: string };

  const handleFileAttach = (nodeId: string, files: FileList | null) => {
    if (!files || files.length === 0) return;
    const currentAttachments: Attachment[] = (getNodeConfig(nodes.find((n) => n.id === nodeId)!).attachments as Attachment[]) || [];
    const promises = Array.from(files).map(
      (file) =>
        new Promise<Attachment>((resolve) => {
          const reader = new FileReader();
          reader.onload = () => {
            const base64 = (reader.result as string).split(',')[1];
            resolve({ data: base64, media_type: file.type || 'application/octet-stream', name: file.name });
          };
          reader.readAsDataURL(file);
        }),
    );
    Promise.all(promises).then((newAttachments) => {
      updateNodeConfig(nodeId, 'attachments', [...currentAttachments, ...newAttachments]);
    });
  };

  const removeAttachment = (nodeId: string, index: number) => {
    const currentAttachments: Attachment[] = (getNodeConfig(nodes.find((n) => n.id === nodeId)!).attachments as Attachment[]) || [];
    updateNodeConfig(nodeId, 'attachments', currentAttachments.filter((_, i) => i !== index));
  };

  const renderAttachments = (nodeId: string, config: NodeConfig) => {
    const attachments = (config.attachments as Attachment[]) || [];
    return (
      <div>
        <label className="block text-xs text-muted-foreground mb-1">Attachments</label>
        {attachments.length > 0 && (
          <div className="space-y-1 mb-2">
            {attachments.map((a, i) => (
              <div key={i} className="flex items-center gap-1.5 text-xs bg-surface rounded px-2 py-1">
                <span className="truncate flex-1" title={a.name}>
                  {a.media_type.startsWith('image/') ? '🖼' : '📎'} {a.name}
                </span>
                <span className="text-muted-foreground shrink-0">
                  {Math.round(a.data.length * 0.75 / 1024)}KB
                </span>
                <button
                  onClick={() => removeAttachment(nodeId, i)}
                  className="text-muted-foreground hover:text-red-500 shrink-0"
                >
                  ✕
                </button>
              </div>
            ))}
          </div>
        )}
        <label className="inline-flex items-center gap-1 px-2 py-1 text-xs border border-dashed border-border rounded cursor-pointer hover:bg-surface">
          <span>+ Add files</span>
          <input
            type="file"
            multiple
            accept="image/*,.pdf,.txt,.csv,.json,.md"
            onChange={(e) => { handleFileAttach(nodeId, e.target.files); e.target.value = ''; }}
            className="hidden"
          />
        </label>
        <p className="text-[10px] text-muted-foreground mt-0.5">
          Images, PDF, text files. Sent as base64 to the model.
        </p>
      </div>
    );
  };

  // Render config editor fields for the selected node
  const renderNodeConfigEditor = () => {
    if (!selectedNode || selectedNode.type === 'start' || selectedNode.type === 'end') return null;
    const config = getNodeConfig(selectedNode);
    const id = selectedNode.id;

    if (selectedNode.type === 'llm') {
      return (
        <div className="space-y-3 mt-3">
          <h4 className="text-xs font-medium text-muted-foreground uppercase tracking-wide">LLM Config</h4>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Provider</label>
            <select
              value={(config.provider as string) || 'gemini'}
              onChange={(e) => {
                const newProvider = e.target.value;
                updateNodeConfig(id, 'provider', newProvider);
                const models = MODEL_OPTIONS[newProvider];
                if (models?.[0]) updateNodeConfig(id, 'model', models[0].id);
              }}
              className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
            >
              <option value="gemini">Gemini</option>
              <option value="claude">Claude</option>
              <option value="openai">OpenAI</option>
            </select>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Model</label>
            <select
              value={(config.model as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'model', e.target.value)}
              className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
            >
              {(MODEL_OPTIONS[(config.provider as string) || 'gemini'] || []).map((m) => (
                <option key={m.id} value={m.id}>{m.label}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">System Prompt</label>
            <textarea
              value={(config.prompt as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'prompt', e.target.value)}
              rows={3}
              placeholder="You are a helpful assistant..."
              className="w-full px-2 py-1.5 border border-border rounded text-xs resize-none"
            />
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Temperature</label>
            <input
              type="number"
              min={0} max={2} step={0.1}
              value={(config.temperature as number) ?? 0.7}
              onChange={(e) => updateNodeConfig(id, 'temperature', parseFloat(e.target.value))}
              className="w-full px-2 py-1.5 border border-border rounded text-xs"
            />
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-xs text-muted-foreground mb-0.5">Input Ch.</label>
              <input
                type="text"
                value={(config.input_channel as string) || 'value'}
                onChange={(e) => updateNodeConfig(id, 'input_channel', e.target.value)}
                className="w-full px-2 py-1.5 border border-border rounded text-xs"
              />
            </div>
            <div>
              <label className="block text-xs text-muted-foreground mb-0.5">Output Ch.</label>
              <input
                type="text"
                value={(config.output_channel as string) || 'value'}
                onChange={(e) => updateNodeConfig(id, 'output_channel', e.target.value)}
                className="w-full px-2 py-1.5 border border-border rounded text-xs"
              />
            </div>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Response Format</label>
            <select
              value={(() => {
                const rf = config.response_format as Record<string, unknown> | undefined;
                if (!rf) return 'text';
                return (rf.type as string) || 'text';
              })()}
              onChange={(e) => {
                const val = e.target.value;
                if (val === 'text') {
                  updateNodeConfig(id, 'response_format', undefined);
                } else if (val === 'json_object') {
                  updateNodeConfig(id, 'response_format', { type: 'json_object' });
                } else if (val === 'json_schema') {
                  updateNodeConfig(id, 'response_format', {
                    type: 'json_schema',
                    name: 'output',
                    schema: {},
                    strict: true,
                  });
                }
              }}
              className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
            >
              <option value="text">Text (default)</option>
              <option value="json_object">JSON Object</option>
              <option value="json_schema">JSON Schema</option>
            </select>
          </div>
          {(() => {
            const rf = config.response_format as Record<string, unknown> | undefined;
            if (rf?.type !== 'json_schema') return null;
            return (
              <div className="space-y-2">
                <div>
                  <label className="block text-xs text-muted-foreground mb-0.5">Schema Name</label>
                  <input
                    type="text"
                    value={(rf.name as string) || 'output'}
                    onChange={(e) => updateNodeConfig(id, 'response_format', { ...rf, name: e.target.value })}
                    className="w-full px-2 py-1.5 border border-border rounded text-xs"
                  />
                </div>
                <div>
                  <label className="block text-xs text-muted-foreground mb-0.5">JSON Schema</label>
                  <textarea
                    value={typeof rf.schema === 'object' ? JSON.stringify(rf.schema, null, 2) : '{}'}
                    onChange={(e) => {
                      try {
                        const schema = JSON.parse(e.target.value);
                        updateNodeConfig(id, 'response_format', { ...rf, schema });
                      } catch {
                        // Don't update on invalid JSON
                      }
                    }}
                    rows={4}
                    placeholder='{"type": "object", "properties": {...}}'
                    className="w-full px-2 py-1.5 border border-border rounded text-xs font-mono resize-none"
                  />
                </div>
              </div>
            );
          })()}
          <div>
            <label className="block text-xs text-muted-foreground mb-1">Tools</label>
            <div className="space-y-1">
              {AVAILABLE_TOOLS.map((tool) => {
                const enabled = ((config.tools as string[]) || []).includes(tool);
                return (
                  <label key={tool} className="flex items-center gap-2 text-xs cursor-pointer">
                    <input
                      type="checkbox"
                      checked={enabled}
                      onChange={(e) => {
                        const current = ((config.tools as string[]) || []).slice();
                        if (e.target.checked) {
                          current.push(tool);
                        } else {
                          const idx = current.indexOf(tool);
                          if (idx >= 0) current.splice(idx, 1);
                        }
                        updateNodeConfig(id, 'tools', current);
                      }}
                      className="accent-blue-500"
                    />
                    <span>{tool}</span>
                  </label>
                );
              })}
            </div>
          </div>
          {((config.tools as string[]) || []).length > 0 && (
            <div>
              <label className="block text-xs text-muted-foreground mb-0.5">Max Tool Iterations</label>
              <input
                type="number"
                min={1} max={20} step={1}
                value={(config.max_tool_iterations as number) ?? 5}
                onChange={(e) => updateNodeConfig(id, 'max_tool_iterations', parseInt(e.target.value) || 5)}
                className="w-full px-2 py-1.5 border border-border rounded text-xs"
              />
            </div>
          )}
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Max Retries</label>
            <input
              type="number"
              min={0} max={5} step={1}
              value={(config.max_retries as number) ?? 0}
              onChange={(e) => updateNodeConfig(id, 'max_retries', parseInt(e.target.value) || 0)}
              className="w-full px-2 py-1.5 border border-border rounded text-xs"
            />
            <p className="text-[10px] text-muted-foreground mt-0.5">
              Retry on failure with exponential backoff (0 = no retry)
            </p>
          </div>
          {renderAttachments(id, config)}
        </div>
      );
    }

    if (selectedNode.type === 'agent') {
      return (
        <div className="space-y-3 mt-3">
          <h4 className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Agent Config</h4>
          <div className="px-2 py-1.5 bg-emerald-50 border border-emerald-200 rounded text-xs text-emerald-700">
            ReAct agent with tool calling loop. Automatically calls tools until done.
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Provider</label>
            <select
              value={(config.provider as string) || 'gemini'}
              onChange={(e) => {
                const newProvider = e.target.value;
                updateNodeConfig(id, 'provider', newProvider);
                const models = MODEL_OPTIONS[newProvider];
                if (models?.[0]) updateNodeConfig(id, 'model', models[0].id);
              }}
              className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
            >
              <option value="gemini">Gemini</option>
              <option value="claude">Claude</option>
              <option value="openai">OpenAI</option>
            </select>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Model</label>
            <select
              value={(config.model as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'model', e.target.value)}
              className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
            >
              {(MODEL_OPTIONS[(config.provider as string) || 'gemini'] || []).map((m) => (
                <option key={m.id} value={m.id}>{m.label}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">System Prompt</label>
            <textarea
              value={(config.system_prompt as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'system_prompt', e.target.value)}
              rows={3}
              placeholder="You are a helpful assistant with access to tools..."
              className="w-full px-2 py-1.5 border border-border rounded text-xs resize-none"
            />
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-1">Tools</label>
            <div className="space-y-1">
              {AVAILABLE_TOOLS.map((tool) => {
                const enabled = ((config.tools as string[]) || []).includes(tool);
                return (
                  <label key={tool} className="flex items-center gap-2 text-xs cursor-pointer">
                    <input
                      type="checkbox"
                      checked={enabled}
                      onChange={(e) => {
                        const current = ((config.tools as string[]) || []).slice();
                        if (e.target.checked) {
                          current.push(tool);
                        } else {
                          const idx = current.indexOf(tool);
                          if (idx >= 0) current.splice(idx, 1);
                        }
                        updateNodeConfig(id, 'tools', current);
                      }}
                      className="accent-emerald-500"
                    />
                    <span>{tool}</span>
                  </label>
                );
              })}
            </div>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Recursion Limit</label>
            <input
              type="number"
              min={1} max={100} step={1}
              value={(config.recursion_limit as number) ?? 25}
              onChange={(e) => updateNodeConfig(id, 'recursion_limit', parseInt(e.target.value) || 25)}
              className="w-full px-2 py-1.5 border border-border rounded text-xs"
            />
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Max Retries</label>
            <input
              type="number"
              min={0} max={5} step={1}
              value={(config.max_retries as number) ?? 0}
              onChange={(e) => updateNodeConfig(id, 'max_retries', parseInt(e.target.value) || 0)}
              className="w-full px-2 py-1.5 border border-border rounded text-xs"
            />
            <p className="text-[10px] text-muted-foreground mt-0.5">
              Retry on failure with exponential backoff (0 = no retry)
            </p>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-xs text-muted-foreground mb-0.5">Input Ch.</label>
              <input
                type="text"
                value={(config.input_channel as string) || 'value'}
                onChange={(e) => updateNodeConfig(id, 'input_channel', e.target.value)}
                className="w-full px-2 py-1.5 border border-border rounded text-xs"
              />
            </div>
            <div>
              <label className="block text-xs text-muted-foreground mb-0.5">Output Ch.</label>
              <input
                type="text"
                value={(config.output_channel as string) || 'value'}
                onChange={(e) => updateNodeConfig(id, 'output_channel', e.target.value)}
                className="w-full px-2 py-1.5 border border-border rounded text-xs"
              />
            </div>
          </div>
          {renderAttachments(id, config)}
        </div>
      );
    }

    if (selectedNode.type === 'deep_research') {
      return (
        <div className="space-y-3 mt-3">
          <h4 className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Deep Research Config</h4>
          <div className="px-2 py-1.5 bg-purple-50 border border-purple-200 rounded text-xs text-purple-700">
            Gemini API Key required. Execution may take several minutes.
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Agent</label>
            <select
              value={(config.agent as string) || 'deep-research-pro-preview-12-2025'}
              onChange={(e) => updateNodeConfig(id, 'agent', e.target.value)}
              className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
            >
              {DEEP_RESEARCH_AGENT_OPTIONS.map((a) => (
                <option key={a.id} value={a.id}>{a.label}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Prompt Template</label>
            <textarea
              value={(config.prompt as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'prompt', e.target.value)}
              rows={3}
              placeholder="{INPUT} to reference input channel value"
              className="w-full px-2 py-1.5 border border-border rounded text-xs resize-none"
            />
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-xs text-muted-foreground mb-0.5">Input Ch.</label>
              <input
                type="text"
                value={(config.input_channel as string) || 'value'}
                onChange={(e) => updateNodeConfig(id, 'input_channel', e.target.value)}
                className="w-full px-2 py-1.5 border border-border rounded text-xs"
              />
            </div>
            <div>
              <label className="block text-xs text-muted-foreground mb-0.5">Output Ch.</label>
              <input
                type="text"
                value={(config.output_channel as string) || 'value'}
                onChange={(e) => updateNodeConfig(id, 'output_channel', e.target.value)}
                className="w-full px-2 py-1.5 border border-border rounded text-xs"
              />
            </div>
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Attachments (text)</label>
            <textarea
              value={(config.attachments_text as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'attachments_text', e.target.value)}
              rows={4}
              placeholder="Paste reference text here (e.g. specs, docs)..."
              className="w-full px-2 py-1.5 border border-border rounded text-xs font-mono resize-y"
            />
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Attachments Ch.</label>
            <input
              type="text"
              value={(config.attachments_channel as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'attachments_channel', e.target.value)}
              placeholder="(optional, read from state)"
              className="w-full px-2 py-1.5 border border-border rounded text-xs"
            />
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">File Search Stores</label>
            <textarea
              value={(config.file_search_store_names as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'file_search_store_names', e.target.value)}
              rows={2}
              placeholder="fileSearchStores/xxx (1 per line)"
              className="w-full px-2 py-1.5 border border-border rounded text-xs font-mono resize-y"
            />
            <p className="text-[10px] text-muted-foreground mt-0.5">Pre-created File Search Store names, one per line</p>
          </div>
        </div>
      );
    }

    if (selectedNode.type === 'interrupt') {
      return (
        <div className="space-y-3 mt-3">
          <h4 className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Interrupt Config</h4>
          <div className="px-2 py-1.5 bg-amber-50 border border-amber-200 rounded text-xs text-amber-700">
            Pauses execution and waits for human input. Requires HITL mode.
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Interrupt Message</label>
            <textarea
              value={(config.value as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'value', e.target.value)}
              rows={3}
              placeholder="Please review and approve this result..."
              className="w-full px-2 py-1.5 border border-border rounded text-xs resize-none"
            />
            <p className="text-[10px] text-muted-foreground mt-0.5">
              Shown to the user when execution pauses at this node.
            </p>
          </div>
        </div>
      );
    }

    if (selectedNode.type === 'transform') {
      return (
        <div className="space-y-3 mt-3">
          <h4 className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Transform Config</h4>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Expression (Rhai)</label>
            <textarea
              value={(config.expression as string) || ''}
              onChange={(e) => updateNodeConfig(id, 'expression', e.target.value)}
              rows={3}
              placeholder='state.value + " world"'
              className="w-full px-2 py-1.5 border border-border rounded text-xs font-mono resize-none"
            />
          </div>
          <div>
            <label className="block text-xs text-muted-foreground mb-0.5">Output Channel</label>
            <input
              type="text"
              value={(config.output_channel as string) || 'value'}
              onChange={(e) => updateNodeConfig(id, 'output_channel', e.target.value)}
              className="w-full px-2 py-1.5 border border-border rounded text-xs"
            />
          </div>
        </div>
      );
    }

    return null;
  };

  return (
    <div className="flex h-full relative">
      {/* Canvas */}
      <div className="flex-1">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onNodeClick={(_, node) => { setSelectedNodeId(node.id); setSelectedEdgeId(null); }}
          onEdgeClick={(_, edge) => { setSelectedEdgeId(edge.id); setSelectedNodeId(null); }}
          onEdgeContextMenu={(e, edge) => {
            e.preventDefault();
            pushUndo();
            setEdges((eds) => eds.filter((ed) => ed.id !== edge.id));
            if (selectedEdgeId === edge.id) setSelectedEdgeId(null);
          }}
          onPaneClick={() => { setSelectedNodeId(null); setSelectedEdgeId(null); }}
          nodeTypes={nodeTypes}
          fitView
          defaultEdgeOptions={{ style: { strokeWidth: 2 } }}
        >
          <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
          <Controls />
        </ReactFlow>
      </div>

      {/* Property Panel */}
      <aside className="w-[280px] border-l border-border bg-card p-4 overflow-y-auto shrink-0">
        {selectedEdge ? (
          <div className="space-y-4">
            <h3 className="text-sm font-semibold text-foreground">Edge: {selectedEdge.source} &rarr; {selectedEdge.target}</h3>
            <div>
              <label className="flex items-center gap-2 text-xs text-card-foreground cursor-pointer">
                <input
                  type="checkbox"
                  checked={!!(selectedEdge.data as Record<string, unknown>)?.fan_out}
                  onChange={(e) => {
                    const checked = e.target.checked;
                    setEdges((eds) =>
                      eds.map((ed) => {
                        if (ed.id !== selectedEdge.id) return ed;
                        return {
                          ...ed,
                          data: { ...(ed.data as Record<string, unknown>), fan_out: checked },
                          style: checked
                            ? { strokeWidth: 2, strokeDasharray: '6 3', stroke: '#f97316' }
                            : { strokeWidth: 2 },
                          label: checked ? 'parallel' : ((ed.data as Record<string, unknown>)?.condition as string || undefined),
                          labelStyle: checked ? { fill: '#f97316', fontSize: 10, fontWeight: 600 } : undefined,
                        };
                      }),
                    );
                  }}
                  className="accent-orange-500"
                />
                <span className="font-medium">Fan-out (parallel execution)</span>
              </label>
              <p className="text-[10px] text-muted-foreground mt-1 ml-5">
                Edges with fan-out from the same source node run their targets in parallel.
              </p>
            </div>
            <div>
              <label className="flex items-center gap-2 text-xs text-card-foreground cursor-pointer">
                <input
                  type="checkbox"
                  checked={!!(selectedEdge.data as Record<string, unknown>)?.on_error}
                  onChange={(e) => {
                    const checked = e.target.checked;
                    setEdges((eds) =>
                      eds.map((ed) => {
                        if (ed.id !== selectedEdge.id) return ed;
                        return {
                          ...ed,
                          data: { ...(ed.data as Record<string, unknown>), on_error: checked },
                          style: checked
                            ? { strokeWidth: 2, strokeDasharray: '5 3', stroke: '#ef4444' }
                            : { strokeWidth: 2 },
                          label: checked ? 'on error' : ((ed.data as Record<string, unknown>)?.condition as string || undefined),
                          labelStyle: checked ? { fill: '#ef4444', fontSize: 10, fontWeight: 600 } : undefined,
                        };
                      }),
                    );
                  }}
                  className="accent-red-500"
                />
                <span className="font-medium">Error edge</span>
              </label>
              <p className="text-[10px] text-muted-foreground mt-1 ml-5">
                Only follow this edge when the source node fails.
              </p>
            </div>
            {!(selectedEdge.data as Record<string, unknown>)?.fan_out && !(selectedEdge.data as Record<string, unknown>)?.on_error && (
              <div className="space-y-2">
                <div className="flex gap-2">
                  <button
                    onClick={() => {
                      setEdges((eds) =>
                        eds.map((ed) =>
                          ed.id === selectedEdge.id
                            ? { ...ed, data: { ...(ed.data as Record<string, unknown>), condition_mode: 'simple' } }
                            : ed,
                        ),
                      );
                    }}
                    className={`px-2 py-1 text-xs rounded ${
                      ((selectedEdge.data as Record<string, unknown>)?.condition_mode || 'simple') === 'simple'
                        ? 'bg-blue-100 text-blue-700 font-medium'
                        : 'text-muted-foreground hover:bg-surface'
                    }`}
                  >
                    Simple
                  </button>
                  <button
                    onClick={() => {
                      setEdges((eds) =>
                        eds.map((ed) =>
                          ed.id === selectedEdge.id
                            ? { ...ed, data: { ...(ed.data as Record<string, unknown>), condition_mode: 'expression' } }
                            : ed,
                        ),
                      );
                    }}
                    className={`px-2 py-1 text-xs rounded ${
                      (selectedEdge.data as Record<string, unknown>)?.condition_mode === 'expression'
                        ? 'bg-purple-100 text-purple-700 font-medium'
                        : 'text-muted-foreground hover:bg-surface'
                    }`}
                  >
                    Expression
                  </button>
                </div>
                {((selectedEdge.data as Record<string, unknown>)?.condition_mode || 'simple') === 'simple' ? (
                  <div>
                    <label className="block text-xs text-muted-foreground mb-0.5">Condition (state key)</label>
                    <input
                      type="text"
                      value={((selectedEdge.data as Record<string, unknown>)?.condition as string) || ''}
                      onChange={(e) => {
                        const val = e.target.value;
                        setEdges((eds) =>
                          eds.map((ed) => {
                            if (ed.id !== selectedEdge.id) return ed;
                            return {
                              ...ed,
                              data: { ...(ed.data as Record<string, unknown>), condition: val || undefined },
                              label: val || undefined,
                            };
                          }),
                        );
                      }}
                      placeholder="e.g. flag"
                      className="w-full px-2 py-1.5 border border-border rounded text-xs"
                    />
                    <p className="text-[10px] text-muted-foreground mt-0.5">
                      Route to this target when state[key] is truthy.
                    </p>
                  </div>
                ) : (
                  <div>
                    <label className="block text-xs text-muted-foreground mb-0.5">Rhai Expression</label>
                    <textarea
                      value={((selectedEdge.data as Record<string, unknown>)?.condition as string) || ''}
                      onChange={(e) => {
                        const val = e.target.value;
                        setEdges((eds) =>
                          eds.map((ed) => {
                            if (ed.id !== selectedEdge.id) return ed;
                            return {
                              ...ed,
                              data: { ...(ed.data as Record<string, unknown>), condition: val || undefined },
                              label: val ? `[${val.slice(0, 20)}${val.length > 20 ? '...' : ''}]` : undefined,
                            };
                          }),
                        );
                      }}
                      rows={2}
                      placeholder='state.score > 50 && state.category == "A"'
                      className="w-full px-2 py-1.5 border border-border rounded text-xs font-mono resize-none"
                    />
                    <p className="text-[10px] text-muted-foreground mt-0.5">
                      Rhai expression. Access state via <code className="bg-surface px-0.5 rounded">state.key</code>. Must return true/false.
                    </p>
                  </div>
                )}
              </div>
            )}
            <button
              onClick={() => {
                setEdges((eds) => eds.filter((ed) => ed.id !== selectedEdge.id));
                setSelectedEdgeId(null);
              }}
              className="text-xs text-red-500 hover:text-red-700"
            >
              Delete edge
            </button>
          </div>
        ) : selectedNode && selectedNode.type !== 'start' && selectedNode.type !== 'end' ? (
          <div className="space-y-4">
            <h3 className="text-sm font-semibold text-foreground">Node: {selectedNode.id}</h3>
            {stateHistory.size > 0 && (
              <div className="flex gap-1 border-b border-border pb-1">
                <button
                  onClick={() => setInspectorTab('config')}
                  className={`px-2 py-1 text-xs rounded-t ${inspectorTab === 'config' ? 'bg-blue-100 text-blue-700 font-medium' : 'text-muted-foreground'}`}
                >
                  Config
                </button>
                <button
                  onClick={() => setInspectorTab('state')}
                  className={`px-2 py-1 text-xs rounded-t ${inspectorTab === 'state' ? 'bg-green-100 text-green-700 font-medium' : 'text-muted-foreground'}`}
                >
                  State {stateHistory.has(selectedNode.id) ? `(${stateHistory.get(selectedNode.id)!.length})` : ''}
                </button>
              </div>
            )}
            {(inspectorTab === 'config' || stateHistory.size === 0) ? (
              <>
                <div>
                  <label className="block text-xs font-medium text-muted-foreground uppercase mb-1">Type</label>
                  <p className="text-sm text-card-foreground capitalize">{selectedNode.type}</p>
                </div>
                <div>
                  <label className="block text-xs font-medium text-muted-foreground uppercase mb-1">Label</label>
                  <input
                    type="text"
                    value={(selectedNode.data as Record<string, string>).label || ''}
                    onChange={(e) => {
                      const val = e.target.value;
                      setNodes((nds) =>
                        nds.map((n) =>
                          n.id === selectedNode.id
                            ? { ...n, data: { ...n.data, label: val } }
                            : n,
                        ),
                      );
                    }}
                    className="w-full px-3 py-2 border border-border rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-ring"
                  />
                </div>
                {renderNodeConfigEditor()}
              </>
            ) : (
              <div className="space-y-2">
                <h4 className="text-xs font-medium text-muted-foreground uppercase tracking-wide">State after execution</h4>
                {stateHistory.has(selectedNode.id) ? (
                  stateHistory.get(selectedNode.id)!.map((entry, i) => (
                    <div key={i} className="space-y-1">
                      <div className="text-xs text-muted-foreground font-medium">Step #{entry.step}</div>
                      <pre className="text-xs font-mono bg-surface rounded p-2 overflow-x-auto max-h-[300px] overflow-y-auto text-foreground">
                        {JSON.stringify(entry.state, null, 2)}
                      </pre>
                    </div>
                  ))
                ) : (
                  <p className="text-xs text-muted-foreground">No state captured for this node</p>
                )}
              </div>
            )}
          </div>
        ) : (
          <div>
            <h3 className="text-sm font-semibold text-foreground mb-2">
              {selectedNode ? `${selectedNode.type?.toUpperCase()} node` : 'No node selected'}
            </h3>
            {!selectedNode && (
              <p className="text-xs text-muted-foreground">Click a node or edge to edit properties</p>
            )}
          </div>
        )}

        <hr className="border-border my-4" />

        <div>
          <h3 className="text-xs font-medium text-muted-foreground uppercase tracking-wide mb-2">Channels</h3>
          <div className="space-y-2">
            {channels.map((ch, i) => (
              <div key={i} className="flex gap-1.5 items-center">
                <input
                  type="text"
                  value={ch.key}
                  onChange={(e) => {
                    const updated = [...channels];
                    updated[i] = { ...updated[i], key: e.target.value };
                    setChannels(updated);
                  }}
                  className="flex-1 px-2 py-1 border border-border rounded text-xs focus:outline-none focus:ring-1 focus:ring-ring"
                  placeholder="key"
                />
                <select
                  value={ch.type}
                  onChange={(e) => {
                    const updated = [...channels];
                    updated[i] = { ...updated[i], type: e.target.value };
                    setChannels(updated);
                  }}
                  className="px-2 py-1 border border-border rounded text-xs bg-card"
                >
                  <option value="LastValue">LastValue</option>
                  <option value="Append">Append</option>
                </select>
                <button
                  onClick={() => setChannels(channels.filter((_, j) => j !== i))}
                  className="text-muted-foreground hover:text-red-500 text-xs"
                >
                  ✕
                </button>
              </div>
            ))}
            <button
              onClick={() => setChannels([...channels, { key: '', type: 'LastValue' }])}
              className="text-xs text-blue-600 hover:text-blue-800"
            >
              + Add channel
            </button>
          </div>
        </div>
      </aside>

      {/* Current graph name */}
      {currentGraphName && (
        <div className="absolute top-4 left-4 px-3 py-1.5 bg-card/90 border border-border rounded-lg text-sm text-foreground shadow-sm backdrop-blur-sm">
          {currentGraphName}
        </div>
      )}

      {/* Toolbar */}
      <div className="absolute bottom-4 left-4 flex gap-2">
        <button
          onClick={handleUndo}
          disabled={undoStack.current.length === 0}
          className="px-3 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface disabled:opacity-30"
          title="Undo (Ctrl+Z)"
        >
          ↩
        </button>
        <button
          onClick={handleRedo}
          disabled={redoStack.current.length === 0}
          className="px-3 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface disabled:opacity-30"
          title="Redo (Ctrl+Shift+Z)"
        >
          ↪
        </button>
        <button
          onClick={() => setAddNodeModal(true)}
          className="px-4 py-2 bg-primary text-primary-foreground text-sm rounded-lg hover:opacity-90"
        >
          + Node
        </button>
        <button
          onClick={handleValidate}
          className="px-4 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface"
        >
          Validate
        </button>
        <button
          onClick={() => setRunModal(true)}
          className="px-4 py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700"
        >
          Run
        </button>
        <button
          onClick={openSaveModal}
          className="px-4 py-2 bg-emerald-600 text-white text-sm rounded-lg hover:bg-emerald-700"
        >
          Save
        </button>
        <button
          onClick={openLoadModal}
          className="px-4 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface"
        >
          Load
        </button>
        <span className="w-px h-6 bg-border self-center" />
        <button
          onClick={() => setTemplateModal(true)}
          className="px-4 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface"
        >
          Templates
        </button>
        <button
          onClick={() => setGenerateModal(true)}
          className="px-4 py-2 bg-purple-600 text-white text-sm rounded-lg hover:bg-purple-700"
        >
          AI Generate
        </button>
        <button
          onClick={handleAutoLayout}
          className="px-4 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface"
        >
          Layout
        </button>
        <button
          onClick={handleExport}
          className="px-4 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface"
        >
          Export
        </button>
        <button
          onClick={() => importInputRef.current?.click()}
          className="px-4 py-2 bg-card border border-border text-sm rounded-lg hover:bg-surface"
        >
          Import
        </button>
        <input
          ref={importInputRef}
          type="file"
          accept=".json"
          onChange={handleImport}
          className="hidden"
        />
      </div>

      {/* Toast */}
      {toast && (
        <div className={`absolute top-4 right-[300px] px-4 py-2 rounded-lg text-sm shadow-lg ${
          toast.type === 'success' ? 'bg-green-100 text-green-800 border border-green-300' : 'bg-red-100 text-red-800 border border-red-300'
        }`}>
          {toast.message}
        </div>
      )}

      {/* Add Node Modal */}
      {addNodeModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setAddNodeModal(false)}>
          <div className="bg-card rounded-lg shadow-xl w-full max-w-sm p-6" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-foreground mb-4">Add Node</h2>
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-card-foreground mb-1">Type</label>
                <select
                  value={newNodeType}
                  onChange={(e) => setNewNodeType(e.target.value)}
                  className="w-full px-3 py-2 border border-border rounded-md text-sm bg-card"
                >
                  <option value="llm">💬 LLM Call</option>
                  <option value="agent">&#x1F916; Agent (ReAct)</option>
                  <option value="interrupt">&#x23F8; Interrupt (HITL)</option>
                  <option value="deep_research">🔬 Deep Research</option>
                  <option value="conditional">◇ Conditional</option>
                  <option value="transform">⚙ Transform</option>
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-card-foreground mb-1">Label</label>
                <input
                  type="text"
                  value={newNodeLabel}
                  onChange={(e) => setNewNodeLabel(e.target.value)}
                  placeholder="e.g. summarize"
                  className="w-full px-3 py-2 border border-border rounded-md text-sm"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-4">
              <button onClick={() => setAddNodeModal(false)} className="px-4 py-2 text-sm text-card-foreground">Cancel</button>
              <button onClick={handleAddNode} className="px-4 py-2 text-sm bg-primary text-primary-foreground rounded-md">Add</button>
            </div>
          </div>
        </div>
      )}

      {/* Run Modal */}
      {runModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => !running && setRunModal(false)}>
          <div className="bg-card rounded-lg shadow-xl w-full max-w-lg p-6 max-h-[80vh] overflow-y-auto" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-foreground mb-4">Execute Graph</h2>
            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-card-foreground mb-1">Input JSON</label>
                <textarea
                  value={runInput}
                  onChange={(e) => setRunInput(e.target.value)}
                  rows={4}
                  className="w-full px-3 py-2 border border-border rounded-md text-sm font-mono resize-none"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-card-foreground mb-1">Recursion Limit</label>
                <input
                  type="number"
                  min={1} max={200} step={1}
                  value={recursionLimit}
                  onChange={(e) => setRecursionLimit(parseInt(e.target.value) || 25)}
                  className="w-full px-3 py-2 border border-border rounded-md text-sm"
                />
                <p className="text-xs text-muted-foreground mt-0.5">Maximum execution steps (prevents infinite loops)</p>
              </div>
              <label className="flex items-center gap-2 text-sm text-card-foreground cursor-pointer">
                <input
                  type="checkbox"
                  checked={hitlMode}
                  onChange={(e) => setHitlMode(e.target.checked)}
                  className="accent-amber-500"
                />
                <span className="font-medium">HITL Mode</span>
                <span className="text-xs text-muted-foreground">(Human-in-the-Loop)</span>
              </label>
              {hitlMode && (
                <p className="text-xs text-amber-600 -mt-2 ml-6">
                  Execution will pause at Interrupt nodes and wait for your input before continuing.
                </p>
              )}
              <button
                onClick={handleRun}
                disabled={running || !!hitlSessionId}
                className="w-full px-4 py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700 disabled:opacity-50"
              >
                {running ? 'Running...' : hitlMode ? '▶ Execute (HITL)' : '▶ Execute'}
              </button>

              {/* HITL Interrupted - Resume Form */}
              {hitlSessionId && (
                <div className="p-3 bg-amber-50 border border-amber-300 rounded-lg space-y-3">
                  <div className="flex items-center gap-2">
                    <span className="text-lg">&#x23F8;</span>
                    <h3 className="text-sm font-semibold text-amber-800">Execution Paused</h3>
                  </div>
                  <div className="text-xs text-amber-700 bg-amber-100 rounded p-2">
                    <span className="font-medium">Interrupt message:</span>
                    <pre className="mt-1 whitespace-pre-wrap">{hitlInterruptValue}</pre>
                  </div>
                  <div>
                    <label className="block text-xs font-medium text-amber-800 mb-1">Your Response</label>
                    <textarea
                      value={hitlResumeInput}
                      onChange={(e) => setHitlResumeInput(e.target.value)}
                      rows={3}
                      placeholder="Type your response or paste JSON..."
                      className="w-full px-2 py-1.5 border border-amber-300 rounded text-sm resize-none"
                    />
                  </div>
                  <button
                    onClick={handleResume}
                    disabled={running || !hitlResumeInput.trim()}
                    className="w-full px-4 py-2 bg-amber-600 text-white text-sm rounded-lg hover:bg-amber-700 disabled:opacity-50"
                  >
                    {running ? 'Resuming...' : '▶ Resume Execution'}
                  </button>
                </div>
              )}

              {runSteps.length > 0 && (
                <div>
                  <h3 className="text-sm font-medium text-card-foreground mb-2">Steps</h3>
                  <div className="space-y-1">
                    {runSteps.map((step, i) => (
                      <div key={i} className={`text-xs font-mono rounded px-2 py-1 ${
                        step.type === 'interrupted' ? 'bg-amber-100 text-amber-800' : 'bg-surface text-card-foreground'
                      }`}>
                        {step.type}: {step.node_id || ''} {step.step_number !== undefined ? `(#${step.step_number})` : ''}
                        {step.type === 'interrupted' ? ' [PAUSED]' : ''}
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {runOutput && (
                <div>
                  <h3 className="text-sm font-medium text-card-foreground mb-2">Output</h3>
                  <pre className="text-xs font-mono bg-surface rounded p-3 overflow-x-auto text-foreground">{runOutput}</pre>
                </div>
              )}
            </div>
            <div className="flex justify-end mt-4">
              <button onClick={() => setRunModal(false)} disabled={running} className="px-4 py-2 text-sm text-card-foreground">
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Save Modal */}
      {saveModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setSaveModal(false)}>
          <div className="bg-card rounded-lg shadow-xl w-full max-w-sm p-6" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-foreground mb-4">
              {currentGraphId ? 'Save Graph' : 'Save New Graph'}
            </h2>
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-card-foreground mb-1">Name</label>
                <input
                  type="text"
                  value={saveName}
                  onChange={(e) => setSaveName(e.target.value)}
                  placeholder="My Graph"
                  className="w-full px-3 py-2 border border-border rounded-md text-sm"
                  onKeyDown={(e) => { if (e.key === 'Enter') handleSave(); }}
                  autoFocus
                />
              </div>
              {currentGraphId && (
                <p className="text-xs text-muted-foreground">
                  Updating existing graph. To save as new, click &quot;Save as New&quot;.
                </p>
              )}
            </div>
            <div className="flex justify-end gap-2 mt-4">
              <button onClick={() => setSaveModal(false)} className="px-4 py-2 text-sm text-card-foreground">Cancel</button>
              <button
                onClick={handleSave}
                disabled={!saveName.trim()}
                className="px-4 py-2 text-sm bg-emerald-600 text-white rounded-md hover:bg-emerald-700 disabled:opacity-50"
              >
                {currentGraphId ? 'Update' : 'Save'}
              </button>
              {currentGraphId && (
                <button
                  onClick={() => {
                    setCurrentGraphId(null);
                    setCurrentGraphName(null);
                    handleSave();
                  }}
                  disabled={!saveName.trim()}
                  className="px-4 py-2 text-sm bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:opacity-50"
                >
                  Save as New
                </button>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Load Modal */}
      {loadModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setLoadModal(false)}>
          <div className="bg-card rounded-lg shadow-xl w-full max-w-md p-6 max-h-[70vh] overflow-y-auto" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-foreground mb-4">Load Graph</h2>
            {savedGraphs.length === 0 ? (
              <p className="text-sm text-muted-foreground py-4 text-center">No saved graphs yet</p>
            ) : (
              <div className="space-y-2">
                {savedGraphs.map((g) => (
                  <div
                    key={g.id}
                    className="flex items-center justify-between px-3 py-2 border border-border rounded-lg hover:bg-surface cursor-pointer group"
                    onClick={() => handleLoad(g.id)}
                  >
                    <div className="flex-1 min-w-0">
                      <p className="text-sm font-medium text-foreground truncate">{g.name}</p>
                      {g.description && (
                        <p className="text-xs text-muted-foreground truncate">{g.description}</p>
                      )}
                      <p className="text-[10px] text-muted-foreground mt-0.5">
                        {new Date(g.updated_at).toLocaleDateString()} {new Date(g.updated_at).toLocaleTimeString()}
                      </p>
                    </div>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDelete(g.id);
                      }}
                      className="ml-2 px-2 py-1 text-xs text-red-500 hover:text-red-700 hover:bg-red-50 rounded opacity-0 group-hover:opacity-100 transition-opacity"
                    >
                      Delete
                    </button>
                  </div>
                ))}
              </div>
            )}
            <div className="flex justify-end mt-4">
              <button onClick={() => setLoadModal(false)} className="px-4 py-2 text-sm text-card-foreground">
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Template Modal */}
      {templateModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setTemplateModal(false)}>
          <div className="bg-card rounded-lg shadow-xl w-full max-w-lg p-6 max-h-[70vh] overflow-y-auto" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-foreground mb-4">Graph Templates</h2>
            <p className="text-xs text-muted-foreground mb-4">Select a template to load a pre-built graph pattern.</p>
            <div className="grid grid-cols-1 gap-3">
              {GRAPH_TEMPLATES.map((t) => (
                <div
                  key={t.id}
                  className="p-4 border border-border rounded-lg hover:bg-surface cursor-pointer transition-colors"
                  onClick={() => handleApplyTemplate(t)}
                >
                  <h3 className="text-sm font-semibold text-foreground">{t.name}</h3>
                  <p className="text-xs text-muted-foreground mt-1">{t.description}</p>
                  <div className="flex gap-2 mt-2">
                    <span className="text-[10px] px-1.5 py-0.5 bg-surface rounded text-muted-foreground">
                      {t.data.nodes.filter((n) => n.type !== 'start' && n.type !== 'end').length} nodes
                    </span>
                    <span className="text-[10px] px-1.5 py-0.5 bg-surface rounded text-muted-foreground">
                      {t.data.edges.length} edges
                    </span>
                    <span className="text-[10px] px-1.5 py-0.5 bg-surface rounded text-muted-foreground">
                      {t.data.channels.length} channels
                    </span>
                  </div>
                </div>
              ))}
            </div>
            <div className="flex justify-end mt-4">
              <button onClick={() => setTemplateModal(false)} className="px-4 py-2 text-sm text-card-foreground">
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {/* AI Generate Modal */}
      {generateModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => !generating && setGenerateModal(false)}>
          <div className="bg-card rounded-lg shadow-xl w-full max-w-md p-6" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-foreground mb-4">AI Generate Graph</h2>
            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-card-foreground mb-1">Describe the graph you want</label>
                <textarea
                  value={generatePrompt}
                  onChange={(e) => setGeneratePrompt(e.target.value)}
                  rows={4}
                  placeholder="e.g. A graph that summarizes text, translates it to Japanese, then validates the translation quality..."
                  className="w-full px-3 py-2 border border-border rounded-md text-sm resize-none"
                  autoFocus
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs text-muted-foreground mb-0.5">Provider</label>
                  <select
                    value={generateProvider}
                    onChange={(e) => {
                      const p = e.target.value as Provider;
                      setGenerateProvider(p);
                      const models = MODEL_OPTIONS[p];
                      if (models?.[0]) setGenerateModel(models[0].id);
                    }}
                    className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
                  >
                    <option value="gemini">Gemini</option>
                    <option value="claude">Claude</option>
                    <option value="openai">OpenAI</option>
                  </select>
                </div>
                <div>
                  <label className="block text-xs text-muted-foreground mb-0.5">Model</label>
                  <select
                    value={generateModel}
                    onChange={(e) => setGenerateModel(e.target.value)}
                    className="w-full px-2 py-1.5 border border-border rounded text-xs bg-card"
                  >
                    {(MODEL_OPTIONS[generateProvider] || []).map((m) => (
                      <option key={m.id} value={m.id}>{m.label}</option>
                    ))}
                  </select>
                </div>
              </div>
              <button
                onClick={handleGenerate}
                disabled={generating || !generatePrompt.trim()}
                className="w-full px-4 py-2 bg-purple-600 text-white text-sm rounded-lg hover:bg-purple-700 disabled:opacity-50"
              >
                {generating ? 'Generating...' : 'Generate Graph'}
              </button>
            </div>
            <div className="flex justify-end mt-4">
              <button onClick={() => setGenerateModal(false)} disabled={generating} className="px-4 py-2 text-sm text-card-foreground">
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
