import type { GraphData } from '../lib/api';

export interface GraphTemplate {
  id: string;
  name: string;
  description: string;
  data: GraphData;
}

export const GRAPH_TEMPLATES: GraphTemplate[] = [
  {
    id: 'react-agent',
    name: 'ReAct Agent',
    description: 'LLM with tools in a reasoning loop. Routes back to tools until done.',
    data: {
      nodes: [
        { id: 'start', type: 'start', position: { x: 250, y: 0 } },
        {
          id: 'agent_1',
          type: 'agent',
          label: 'Agent',
          position: { x: 200, y: 120 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            system_prompt: 'You are a helpful assistant with access to tools.',
            tools: ['calculator', 'datetime'],
            recursion_limit: 25,
            input_channel: 'value',
            output_channel: 'value',
          },
        },
        { id: 'end', type: 'end', position: { x: 250, y: 260 } },
      ],
      edges: [
        { from: 'start', to: 'agent_1' },
        { from: 'agent_1', to: 'end' },
      ],
      channels: [{ key: 'value', type: 'LastValue' }],
      node_counter: 2,
    },
  },
  {
    id: 'plan-execute',
    name: 'Plan & Execute',
    description: 'Planner creates a plan, executor runs it, validator checks results.',
    data: {
      nodes: [
        { id: 'start', type: 'start', position: { x: 250, y: 0 } },
        {
          id: 'llm_1',
          type: 'llm',
          label: 'Planner',
          position: { x: 200, y: 120 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            prompt: 'Create a step-by-step plan for the given task. Output each step on a new line.',
            temperature: 0.3,
            input_channel: 'value',
            output_channel: 'plan',
          },
        },
        {
          id: 'llm_2',
          type: 'llm',
          label: 'Executor',
          position: { x: 200, y: 260 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            prompt: 'Execute the following plan step by step and provide the result.',
            temperature: 0.5,
            input_channel: 'plan',
            output_channel: 'result',
          },
        },
        {
          id: 'llm_3',
          type: 'llm',
          label: 'Validator',
          position: { x: 200, y: 400 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            prompt: 'Review and validate the execution result. Summarize the final answer.',
            temperature: 0.2,
            input_channel: 'result',
            output_channel: 'value',
          },
        },
        { id: 'end', type: 'end', position: { x: 250, y: 540 } },
      ],
      edges: [
        { from: 'start', to: 'llm_1' },
        { from: 'llm_1', to: 'llm_2' },
        { from: 'llm_2', to: 'llm_3' },
        { from: 'llm_3', to: 'end' },
      ],
      channels: [
        { key: 'value', type: 'LastValue' },
        { key: 'plan', type: 'LastValue' },
        { key: 'result', type: 'LastValue' },
      ],
      node_counter: 4,
    },
  },
  {
    id: 'reflection',
    name: 'Reflection',
    description: 'Generator creates content, Critic reviews it, loops back for improvement.',
    data: {
      nodes: [
        { id: 'start', type: 'start', position: { x: 250, y: 0 } },
        {
          id: 'llm_1',
          type: 'llm',
          label: 'Generator',
          position: { x: 200, y: 120 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            prompt: 'Generate a response based on the input. If there is feedback, improve your response accordingly.',
            temperature: 0.7,
            input_channel: 'value',
            output_channel: 'draft',
          },
        },
        {
          id: 'llm_2',
          type: 'llm',
          label: 'Critic',
          position: { x: 200, y: 280 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            prompt: 'Review the draft critically. If it needs improvement, provide specific feedback. If it is good enough, respond with exactly "APPROVED".',
            temperature: 0.3,
            input_channel: 'draft',
            output_channel: 'feedback',
          },
        },
        {
          id: 'conditional_1',
          type: 'conditional',
          label: 'Approved?',
          position: { x: 200, y: 440 },
        },
        { id: 'end', type: 'end', position: { x: 250, y: 580 } },
      ],
      edges: [
        { from: 'start', to: 'llm_1' },
        { from: 'llm_1', to: 'llm_2' },
        { from: 'llm_2', to: 'conditional_1' },
        { from: 'conditional_1', to: 'end', condition: 'feedback' },
        { from: 'conditional_1', to: 'llm_1' },
      ],
      channels: [
        { key: 'value', type: 'LastValue' },
        { key: 'draft', type: 'LastValue' },
        { key: 'feedback', type: 'LastValue' },
      ],
      node_counter: 3,
    },
  },
  {
    id: 'hitl-review',
    name: 'HITL Review',
    description: 'LLM generates content, human reviews and approves, then LLM refines.',
    data: {
      nodes: [
        { id: 'start', type: 'start', position: { x: 250, y: 0 } },
        {
          id: 'llm_1',
          type: 'llm',
          label: 'Drafter',
          position: { x: 200, y: 120 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            prompt: 'Create a draft response for the given input.',
            temperature: 0.7,
            input_channel: 'value',
            output_channel: 'draft',
          },
        },
        {
          id: 'interrupt_1',
          type: 'interrupt',
          label: 'Human Review',
          position: { x: 200, y: 280 },
          config: { value: 'Please review the draft and provide feedback or approve.' },
        },
        {
          id: 'llm_2',
          type: 'llm',
          label: 'Refiner',
          position: { x: 200, y: 440 },
          config: {
            provider: 'gemini',
            model: 'gemini-2.5-flash',
            prompt: 'Refine the draft based on the human feedback provided.',
            temperature: 0.5,
            input_channel: 'draft',
            output_channel: 'value',
          },
        },
        { id: 'end', type: 'end', position: { x: 250, y: 580 } },
      ],
      edges: [
        { from: 'start', to: 'llm_1' },
        { from: 'llm_1', to: 'interrupt_1' },
        { from: 'interrupt_1', to: 'llm_2' },
        { from: 'llm_2', to: 'end' },
      ],
      channels: [
        { key: 'value', type: 'LastValue' },
        { key: 'draft', type: 'LastValue' },
      ],
      node_counter: 3,
    },
  },
];
