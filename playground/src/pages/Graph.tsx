import { useState, useCallback, useRef } from 'react';
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
  type GraphNodeDto,
  type GraphEdgeDto,
  type GraphChannelDto,
  type GraphSseEvent,
} from '../lib/api';

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
      data.active ? 'border-blue-500 bg-blue-50 animate-pulse' : 'border-gray-300 bg-white'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-gray-400" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">💬</span>
        <span className="font-medium text-gray-800">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-gray-400" />
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
        <span className="font-medium text-gray-800">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-yellow-500" />
    </div>
  );
}

function TransformNode({ data }: { data: { label: string; active?: boolean } }) {
  return (
    <div className={`px-4 py-2 border-2 rounded-lg text-sm min-w-[120px] ${
      data.active ? 'border-gray-500 bg-gray-100 animate-pulse' : 'border-gray-300 bg-gray-50'
    }`}>
      <Handle type="target" position={Position.Top} className="!bg-gray-500" />
      <div className="flex items-center gap-1.5">
        <span className="text-base">⚙</span>
        <span className="font-medium text-gray-800">{data.label}</span>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-gray-500" />
    </div>
  );
}

const nodeTypes: NodeTypes = {
  start: StartNode,
  end: EndNode,
  llm: LlmNode,
  conditional: ConditionalNode,
  transform: TransformNode,
  passthrough: TransformNode,
};

// --- Graph Page ---

const INITIAL_NODES: Node[] = [
  { id: 'start', type: 'start', position: { x: 250, y: 0 }, data: {} },
  { id: 'end', type: 'end', position: { x: 250, y: 300 }, data: {} },
];
const INITIAL_EDGES: Edge[] = [];

type ChannelEntry = { key: string; type: string; default?: string };

export default function Graph() {
  const [nodes, setNodes, onNodesChange] = useNodesState(INITIAL_NODES);
  const [edges, setEdges, onEdgesChange] = useEdgesState(INITIAL_EDGES);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [channels, setChannels] = useState<ChannelEntry[]>([
    { key: 'value', type: 'LastValue', default: '' },
  ]);
  const [toast, setToast] = useState<{ type: 'success' | 'error'; message: string } | null>(null);
  const [runModal, setRunModal] = useState(false);
  const [runInput, setRunInput] = useState('{"value": "hello"}');
  const [runOutput, setRunOutput] = useState<string | null>(null);
  const [runSteps, setRunSteps] = useState<GraphSseEvent[]>([]);
  const [running, setRunning] = useState(false);
  const [addNodeModal, setAddNodeModal] = useState(false);
  const [newNodeType, setNewNodeType] = useState('llm');
  const [newNodeLabel, setNewNodeLabel] = useState('');
  const nodeCounter = useRef(1);

  const onConnect: OnConnect = useCallback(
    (params) => setEdges((eds) => addEdge(params, eds)),
    [setEdges],
  );

  const selectedNode = nodes.find((n) => n.id === selectedNodeId);

  const showToast = (type: 'success' | 'error', message: string) => {
    setToast({ type, message });
    setTimeout(() => setToast(null), 4000);
  };

  // Convert ReactFlow nodes/edges to API DTOs
  const toApiNodes = (): GraphNodeDto[] =>
    nodes
      .filter((n) => n.type !== 'start' && n.type !== 'end')
      .map((n) => ({
        id: n.id,
        type: n.type || 'passthrough',
        label: (n.data as Record<string, string>).label || n.id,
        config: (n.data as Record<string, unknown>).config as Record<string, unknown> | undefined,
      }));

  const toApiEdges = (): GraphEdgeDto[] =>
    edges.map((e) => ({
      from: e.source,
      to: e.target,
      condition: e.label as string | undefined,
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

  const handleRun = async () => {
    setRunning(true);
    setRunOutput(null);
    setRunSteps([]);

    let inputJson: unknown;
    try {
      inputJson = JSON.parse(runInput);
    } catch {
      showToast('error', 'Invalid JSON input');
      setRunning(false);
      return;
    }

    try {
      await graphInvokeStream(
        toApiNodes(),
        toApiEdges(),
        toApiChannels(),
        inputJson,
        (event: GraphSseEvent) => {
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
          }
          if (event.type === 'complete' || event.type === 'graph_complete') {
            setRunOutput(JSON.stringify(event.output, null, 2));
          }
          if (event.type === 'error') {
            showToast('error', event.message || 'Execution error');
          }
        },
      );
    } catch (err) {
      showToast('error', err instanceof Error ? err.message : 'Execution failed');
    } finally {
      setRunning(false);
      // Clear active highlights
      setNodes((nds) => nds.map((n) => ({ ...n, data: { ...n.data, active: false } })));
    }
  };

  const handleAddNode = () => {
    if (!newNodeLabel.trim()) return;
    const id = `${newNodeType}_${nodeCounter.current++}`;
    const newNode: Node = {
      id,
      type: newNodeType,
      position: { x: 200 + Math.random() * 100, y: 100 + nodes.length * 60 },
      data: { label: newNodeLabel },
    };
    setNodes((nds) => [...nds, newNode]);
    setNewNodeLabel('');
    setAddNodeModal(false);
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
          onNodeClick={(_, node) => setSelectedNodeId(node.id)}
          onPaneClick={() => setSelectedNodeId(null)}
          nodeTypes={nodeTypes}
          fitView
          defaultEdgeOptions={{ style: { strokeWidth: 2 } }}
        >
          <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
          <Controls />
        </ReactFlow>
      </div>

      {/* Property Panel */}
      <aside className="w-[280px] border-l border-gray-200 bg-white p-4 overflow-y-auto shrink-0">
        {selectedNode && selectedNode.type !== 'start' && selectedNode.type !== 'end' ? (
          <div className="space-y-4">
            <h3 className="text-sm font-semibold text-gray-900">Node: {selectedNode.id}</h3>
            <div>
              <label className="block text-xs font-medium text-gray-500 uppercase mb-1">Type</label>
              <p className="text-sm text-gray-700 capitalize">{selectedNode.type}</p>
            </div>
            <div>
              <label className="block text-xs font-medium text-gray-500 uppercase mb-1">Label</label>
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
                className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
            </div>
          </div>
        ) : (
          <div>
            <h3 className="text-sm font-semibold text-gray-900 mb-2">
              {selectedNode ? `${selectedNode.type?.toUpperCase()} node` : 'No node selected'}
            </h3>
            {!selectedNode && (
              <p className="text-xs text-gray-400">Click a node to edit its properties</p>
            )}
          </div>
        )}

        <hr className="border-gray-200 my-4" />

        <div>
          <h3 className="text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">Channels</h3>
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
                  className="flex-1 px-2 py-1 border border-gray-300 rounded text-xs focus:outline-none focus:ring-1 focus:ring-blue-500"
                  placeholder="key"
                />
                <select
                  value={ch.type}
                  onChange={(e) => {
                    const updated = [...channels];
                    updated[i] = { ...updated[i], type: e.target.value };
                    setChannels(updated);
                  }}
                  className="px-2 py-1 border border-gray-300 rounded text-xs bg-white"
                >
                  <option value="LastValue">LastValue</option>
                  <option value="Append">Append</option>
                </select>
                <button
                  onClick={() => setChannels(channels.filter((_, j) => j !== i))}
                  className="text-gray-400 hover:text-red-500 text-xs"
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

      {/* Toolbar */}
      <div className="absolute bottom-4 left-4 flex gap-2">
        <button
          onClick={() => setAddNodeModal(true)}
          className="px-4 py-2 bg-gray-900 text-white text-sm rounded-lg hover:bg-gray-800"
        >
          + Node
        </button>
        <button
          onClick={handleValidate}
          className="px-4 py-2 bg-white border border-gray-300 text-sm rounded-lg hover:bg-gray-50"
        >
          Validate ✓
        </button>
        <button
          onClick={() => setRunModal(true)}
          className="px-4 py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700"
        >
          ▶ Run
        </button>
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
          <div className="bg-white rounded-lg shadow-xl w-full max-w-sm p-6" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-gray-900 mb-4">Add Node</h2>
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Type</label>
                <select
                  value={newNodeType}
                  onChange={(e) => setNewNodeType(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm bg-white"
                >
                  <option value="llm">💬 LLM Call</option>
                  <option value="conditional">◇ Conditional</option>
                  <option value="transform">⚙ Transform</option>
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Label</label>
                <input
                  type="text"
                  value={newNodeLabel}
                  onChange={(e) => setNewNodeLabel(e.target.value)}
                  placeholder="e.g. summarize"
                  className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-4">
              <button onClick={() => setAddNodeModal(false)} className="px-4 py-2 text-sm text-gray-600">Cancel</button>
              <button onClick={handleAddNode} className="px-4 py-2 text-sm bg-gray-900 text-white rounded-md">Add</button>
            </div>
          </div>
        </div>
      )}

      {/* Run Modal */}
      {runModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => !running && setRunModal(false)}>
          <div className="bg-white rounded-lg shadow-xl w-full max-w-lg p-6 max-h-[80vh] overflow-y-auto" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-gray-900 mb-4">Execute Graph</h2>
            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Input JSON</label>
                <textarea
                  value={runInput}
                  onChange={(e) => setRunInput(e.target.value)}
                  rows={4}
                  className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm font-mono resize-none"
                />
              </div>
              <button
                onClick={handleRun}
                disabled={running}
                className="w-full px-4 py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700 disabled:opacity-50"
              >
                {running ? 'Running...' : '▶ Execute'}
              </button>

              {runSteps.length > 0 && (
                <div>
                  <h3 className="text-sm font-medium text-gray-700 mb-2">Steps</h3>
                  <div className="space-y-1">
                    {runSteps.map((step, i) => (
                      <div key={i} className="text-xs font-mono bg-gray-50 rounded px-2 py-1 text-gray-600">
                        {step.type}: {step.node_id || ''} {step.step_number !== undefined ? `(#${step.step_number})` : ''}
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {runOutput && (
                <div>
                  <h3 className="text-sm font-medium text-gray-700 mb-2">Output</h3>
                  <pre className="text-xs font-mono bg-gray-50 rounded p-3 overflow-x-auto text-gray-800">{runOutput}</pre>
                </div>
              )}
            </div>
            <div className="flex justify-end mt-4">
              <button onClick={() => setRunModal(false)} disabled={running} className="px-4 py-2 text-sm text-gray-600">
                Close
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
