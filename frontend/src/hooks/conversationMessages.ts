import type {
  AssistantBlock,
  ChatMessage,
  ToolArguments,
  ToolCallState,
  ToolCallStatus,
  UiMessage,
  UiMessageItem,
} from '../types';
import { isRecord } from '../utils/isRecord';
import { readToolStartTime } from './conversationStorage';

export function genClientId(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return 'c' + Math.random().toString(36).slice(2, 10);
}

export function isAbortError(cause: unknown): boolean {
  return cause instanceof DOMException && cause.name === 'AbortError';
}

export function toUiMessage(msg: ChatMessage & { role: 'user' | 'assistant' }): UiMessage {
  return { ...msg, clientId: msg.id ?? genClientId() };
}

export function parseJsonObject(value: string): ToolArguments {
  try {
    const parsed = JSON.parse(value);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function statusFromToolResult(result: unknown): ToolCallStatus {
  const status = isRecord(result) ? result.status : undefined;
  if (
    status === 'completed'
    || status === 'running'
    || status === 'blocked'
    || status === 'rejected'
    || status === 'interrupted'
    || status === 'error'
    || status === 'pending_approval'
  ) {
    return status;
  }
  return 'completed';
}

export function isAssistantMessage(item: UiMessageItem): item is UiMessage & { role: 'assistant' } {
  return item.role === 'assistant';
}

export function isUserMessage(item: UiMessageItem): item is UiMessage & { role: 'user' } {
  return item.role === 'user';
}

export function listTools(message: UiMessage): ToolCallState[] {
  return (message.blocks ?? [])
    .filter((block): block is Extract<AssistantBlock, { type: 'tool' }> => block.type === 'tool')
    .map((block) => block.tool);
}

function updateAssistantTools(
  message: UiMessage,
  mapper: (tool: ToolCallState) => ToolCallState,
): UiMessage & { role: 'assistant' } {
  if (!message.blocks?.length) return { ...message, role: 'assistant' };
  let changed = false;
  const blocks = message.blocks.map((block) => {
    if (block.type !== 'tool') return block;
    const nextTool = mapper(block.tool);
    if (nextTool === block.tool) return block;
    changed = true;
    return { type: 'tool' as const, tool: nextTool };
  });
  return changed
    ? { ...message, role: 'assistant', blocks }
    : { ...message, role: 'assistant' };
}

export function upsertToolOnAssistant(
  message: UiMessage,
  tool: ToolCallState,
): UiMessage & { role: 'assistant' } {
  const blocks = [...(message.blocks ?? [])];
  const existingIndex = blocks.findIndex(
    (block) => block.type === 'tool' && block.tool.call_id === tool.call_id,
  );
  if (existingIndex >= 0) {
    const existing = blocks[existingIndex];
    if (existing.type === 'tool') {
      blocks[existingIndex] = {
        type: 'tool',
        tool: {
          ...existing.tool,
          ...tool,
          arguments: { ...existing.tool.arguments, ...tool.arguments },
          arguments_text: tool.arguments_text ?? existing.tool.arguments_text,
          result: tool.result ?? existing.tool.result,
        },
      };
    }
  } else {
    // Promote plain content into an ordered text block before the first tool.
    if (
      message.content
      && !blocks.some((block) => block.type === 'text' || block.type === 'tool')
    ) {
      blocks.push({ type: 'text', content: message.content });
    }
    blocks.push({ type: 'tool', tool });
  }
  return { ...message, role: 'assistant', blocks };
}

export function upsertToolOnMessages(
  items: UiMessageItem[],
  tool: ToolCallState,
  agentId: string,
  conversationId: string,
): UiMessageItem[] {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (isUserMessage(item)) break;
    if (isAssistantMessage(item)) {
      return items.map((entry, entryIndex) => (
        entryIndex === index ? upsertToolOnAssistant(entry, tool) : entry
      ));
    }
  }
  const seed: UiMessage = {
    id: '',
    agent_id: agentId,
    conversation_id: conversationId,
    role: 'assistant',
    content: '',
    status: 'streaming',
    created_at: new Date().toISOString(),
    clientId: genClientId(),
    blocks: [{ type: 'tool', tool }],
  };
  return [...items, seed];
}

export function patchToolOnMessages(
  items: UiMessageItem[],
  callId: string,
  patch: Partial<ToolCallState> | ((tool: ToolCallState) => ToolCallState),
): UiMessageItem[] {
  return items.map((item) => {
    if (!isAssistantMessage(item) || !item.blocks?.length) return item;
    let changed = false;
    const blocks = item.blocks.map((block) => {
      if (block.type !== 'tool' || block.tool.call_id !== callId) return block;
      changed = true;
          const nextTool = typeof patch === 'function'
        ? patch(block.tool)
        : {
          ...block.tool,
          ...patch,
            arguments: patch.arguments
              ? { ...block.tool.arguments, ...patch.arguments }
              : block.tool.arguments,
            arguments_text: patch.arguments_text ?? block.tool.arguments_text,
            result: patch.result ?? block.tool.result,
        };
      return { type: 'tool' as const, tool: nextTool };
    });
    return changed ? { ...item, blocks } : item;
  });
}

export function appendTextDeltaToAssistant(message: UiMessage, delta: string): UiMessage & { role: 'assistant' } {
  const content = (message.content ?? '') + delta;
  const blocks = [...(message.blocks ?? [])];
  const last = blocks[blocks.length - 1];
  if (last && last.type === 'text') {
    blocks[blocks.length - 1] = { type: 'text', content: last.content + delta };
  } else if (blocks.some((block) => block.type === 'tool')) {
    blocks.push({ type: 'text', content: delta });
  }
  return {
    ...message,
    role: 'assistant',
    content,
    blocks: blocks.length ? blocks : message.blocks,
    status: 'streaming',
  };
}

export function assistantHasVisibleBody(message: UiMessage): boolean {
  if (message.content?.trim()) return true;
  if (message.reasoning_content?.trim()) return true;
  if (message.attachments?.length) return true;
  if (listTools(message).length > 0) return true;
  return false;
}

export function interruptActiveRun(
  items: UiMessageItem[],
  reason: string,
  toolStatus: 'rejected' | 'error',
): UiMessageItem[] {
  let activeRunStart = -1;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (isUserMessage(items[index])) {
      activeRunStart = index;
      break;
    }
  }

  const next = items.map((item, index) => {
    if (index <= activeRunStart || !isAssistantMessage(item)) return item;
    return updateAssistantTools(item, (tool) => {
      if (tool.status !== 'running' && tool.status !== 'pending_approval') return tool;
      return {
        ...tool,
        status: tool.status === 'pending_approval' ? 'rejected' as const : toolStatus,
        result: toolStatus === 'error'
          ? { ...tool.result, message: reason }
          : { ...tool.result, reason },
      };
    });
  });

  let assistantIndex = -1;
  for (let index = next.length - 1; index > activeRunStart; index -= 1) {
    const item = next[index];
    if (isAssistantMessage(item) && item.status === 'streaming') {
      assistantIndex = index;
      break;
    }
  }
  if (assistantIndex < 0) return next;
  const assistant = next[assistantIndex];
  if (!isAssistantMessage(assistant)) return next;
  if (!assistantHasVisibleBody(assistant)) {
    return next.filter((_, index) => index !== assistantIndex);
  }
  return next.map((item, index) => (
    index === assistantIndex && isAssistantMessage(item) ? { ...item, status: 'failed' as const } : item
  ));
}

export function toUiMessageItems(raw: ChatMessage[]): UiMessageItem[] {
  const items: UiMessageItem[] = [];
  const toolOwner = new Map<string, number>();
  let lastAssistantIndex = -1;

  for (const msg of raw) {
    if (msg.role === 'assistant') {
      const toolCalls = Array.isArray(msg.tool_calls) ? msg.tool_calls : [];
      const tools: ToolCallState[] = toolCalls.map((call) => ({
        call_id: call.id,
        name: call.function.name,
        arguments: parseJsonObject(call.function.arguments),
        started_at_ms: call.started_at_ms ?? readToolStartTime(call.id),
        permission_mode: call.permission_mode,
        status: 'running' as const,
      }));

      // One bubble per user turn: fold all assistant/tool steps into the latest assistant.
      if (lastAssistantIndex >= 0 && isAssistantMessage(items[lastAssistantIndex])) {
        let merged = items[lastAssistantIndex];
        if (msg.reasoning_content?.trim()) {
          merged = {
            ...merged,
            role: 'assistant',
            reasoning_content: [merged.reasoning_content, msg.reasoning_content]
              .filter(Boolean)
              .join("\n\n"),
          };
        }
        if (msg.content.trim()) {
          const blocks = [...(merged.blocks ?? [])];
          if (
            merged.content
            && !blocks.some((block) => block.type === 'text' || block.type === 'tool')
          ) {
            blocks.push({ type: 'text', content: merged.content });
          }
          const lastBlock = blocks[blocks.length - 1];
          if (lastBlock && lastBlock.type === 'text') {
            blocks[blocks.length - 1] = {
              type: 'text',
              content: `${lastBlock.content}\n\n${msg.content}`,
            };
          } else {
            blocks.push({ type: 'text', content: msg.content });
          }
          merged = {
            ...merged,
            role: 'assistant',
            content: merged.content
              ? `${merged.content}\n\n${msg.content}`
              : msg.content,
            blocks: blocks.length ? blocks : undefined,
            usage: msg.usage ?? merged.usage,
            status: msg.status ?? merged.status,
            id: msg.id || merged.id,
          };
        }
        for (const tool of tools) {
          merged = upsertToolOnAssistant(merged, tool);
          toolOwner.set(tool.call_id, lastAssistantIndex);
        }
        if (msg.usage) merged = { ...merged, role: 'assistant', usage: msg.usage };
        items[lastAssistantIndex] = { ...merged, role: 'assistant' };
        continue;
      }

      const blocks: AssistantBlock[] = [];
      if (msg.content.trim()) {
        blocks.push({ type: 'text', content: msg.content });
      }
      for (const tool of tools) {
        blocks.push({ type: 'tool', tool });
      }

      const ui = toUiMessage({
        ...msg,
        role: 'assistant',
        tool_calls: undefined,
      });
      ui.blocks = blocks.length ? blocks : undefined;
      items.push(ui);
      lastAssistantIndex = items.length - 1;
      for (const tool of tools) {
        toolOwner.set(tool.call_id, lastAssistantIndex);
      }
      continue;
    }

    if (msg.role === 'tool') {
      const result = parseJsonObject(msg.content);
      const callId = msg.tool_call_id ?? msg.id;
      const ownerIndex = toolOwner.get(callId);
      const status = statusFromToolResult(result);
      if (ownerIndex !== undefined) {
        const current = items[ownerIndex];
        if (isAssistantMessage(current)) {
          const existing = listTools(current).find((tool) => tool.call_id === callId);
          items[ownerIndex] = upsertToolOnAssistant(current, {
            call_id: callId,
            name: existing?.name ?? 'tool',
            arguments: existing?.arguments ?? {},
            permission_mode: existing?.permission_mode,
            status,
            result,
          });
        }
      } else if (lastAssistantIndex >= 0 && isAssistantMessage(items[lastAssistantIndex])) {
        items[lastAssistantIndex] = upsertToolOnAssistant(items[lastAssistantIndex], {
          call_id: callId,
          name: 'tool',
          arguments: {},
          status,
          result,
        });
        toolOwner.set(callId, lastAssistantIndex);
      } else {
        const orphan: UiMessage = {
          id: msg.id,
          agent_id: msg.agent_id,
          conversation_id: msg.conversation_id,
          role: 'assistant',
          content: '',
          status: 'done',
          created_at: msg.created_at,
          clientId: msg.id ?? genClientId(),
          blocks: [{
            type: 'tool',
            tool: {
              call_id: callId,
              name: 'tool',
              arguments: {},
              status,
              result,
            },
          }],
        };
        items.push(orphan);
        lastAssistantIndex = items.length - 1;
        toolOwner.set(callId, lastAssistantIndex);
      }
      continue;
    }

    if (msg.role === 'user') {
      items.push(toUiMessage(msg as ChatMessage & { role: 'user' }));
      lastAssistantIndex = -1;
    }
  }

  return items;
}


