import { isRecord } from '../../utils/isRecord';

type JsonRecord = Record<string, unknown>;

const DISPLAY_TRUNCATION_MARKER = '[..., 此处已截断, ...]';

function normalizeTruncationMarker(raw: string): string {
  return raw.replace(
    /\[(?:middle truncated|log field truncated|LLM stream log truncated|LLM stream log middle truncated)\]/g,
    DISPLAY_TRUNCATION_MARKER,
  );
}

export interface FormattedLlmResponse {
  isStreaming: boolean;
  formatted: string;
  raw: string;
}

interface StreamToolCall {
  index: number;
  id?: string;
  type?: string;
  function: {
    name: string;
    arguments: string;
  };
}

interface StreamChoice {
  index: number;
  role: string;
  content: string;
  reasoningContent: string;
  toolCalls: Map<number, StreamToolCall>;
  finishReason?: unknown;
  nativeFinishReason?: unknown;
}

export function formatRawJson(raw: string | undefined): string {
  if (!raw) return '';
  const displayRaw = normalizeTruncationMarker(raw);
  try {
    return JSON.stringify(JSON.parse(displayRaw), null, 2);
  } catch {
    return displayRaw;
  }
}

export function formatLlmResponse(raw: string): FormattedLlmResponse {
  const hasTruncationMarker = /\[(?:middle truncated|log field truncated|LLM stream log truncated|LLM stream log middle truncated)\]/
    .test(raw);
  const displayRaw = normalizeTruncationMarker(raw);
  const payloads = parseSsePayloads(displayRaw);
  if (!payloads) {
    return {
      isStreaming: false,
      formatted: formatRawJson(displayRaw),
      raw: displayRaw,
    };
  }

  const aggregated = aggregateOpenAiChat(payloads)
    ?? aggregateOpenAiResponses(payloads)
    ?? aggregateAnthropic(payloads)
    ?? aggregateGemini(payloads)
    ?? { events: payloads };
  if (hasTruncationMarker) {
    aggregated.truncated = true;
    aggregated.truncation_notice = DISPLAY_TRUNCATION_MARKER;
  }

  return {
    isStreaming: true,
    formatted: JSON.stringify(aggregated, null, 2),
    raw: displayRaw,
  };
}

function parseSsePayloads(raw: string): JsonRecord[] | null {
  const payloads: JsonRecord[] = [];
  let isStreaming = false;

  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith(':') || trimmed.startsWith('event:')) {
      isStreaming = true;
      continue;
    }
    if (!trimmed.startsWith('data:')) continue;

    isStreaming = true;
    const data = trimmed.slice(5).trim();
    if (!data || data === '[DONE]') continue;
    try {
      const parsed = JSON.parse(data) as unknown;
      if (isRecord(parsed)) payloads.push(parsed);
    } catch {
      // The original stream remains available below when a provider emits non-JSON data.
    }
  }

  return isStreaming && payloads.length > 0 ? payloads : null;
}

function aggregateOpenAiChat(payloads: JsonRecord[]): JsonRecord | null {
  const chatPayloads = payloads.filter((payload) => Array.isArray(payload.choices)
    && (typeof payload.object === 'string' && payload.object.includes('chat.completion')
      || (payload.choices as unknown[]).some((choice) => isRecord(choice) && isRecord(choice.delta))));
  if (chatPayloads.length === 0) return null;

  const choices = new Map<number, StreamChoice>();
  for (const payload of chatPayloads) {
    for (const rawChoice of payload.choices as unknown[]) {
      if (!isRecord(rawChoice)) continue;
      const index = typeof rawChoice.index === 'number' ? rawChoice.index : 0;
      const choice = choices.get(index) ?? {
        index,
        role: 'assistant',
        content: '',
        reasoningContent: '',
        toolCalls: new Map<number, StreamToolCall>(),
      };
      const delta = isRecord(rawChoice.delta) ? rawChoice.delta : {};
      if (typeof delta.role === 'string') choice.role = delta.role;
      if (typeof delta.content === 'string') choice.content += delta.content;
      if (typeof delta.reasoning_content === 'string') {
        choice.reasoningContent += delta.reasoning_content;
      }
      appendOpenAiToolCalls(choice.toolCalls, delta.tool_calls);
      if (rawChoice.finish_reason != null) choice.finishReason = rawChoice.finish_reason;
      if (rawChoice.native_finish_reason != null) {
        choice.nativeFinishReason = rawChoice.native_finish_reason;
      }
      choices.set(index, choice);
    }
  }

  const first = chatPayloads[0];
  const last = chatPayloads[chatPayloads.length - 1];
  const result: JsonRecord = {};
  copyDefined(result, first, ['id', 'created', 'model', 'provider']);
  result.object = typeof first.object === 'string'
    ? first.object.replace(/\.chunk$/, '')
    : 'chat.completion';
  result.choices = [...choices.values()]
    .sort((a, b) => a.index - b.index)
    .map((choice) => {
      const message: JsonRecord = {
        role: choice.role,
        content: choice.content,
      };
      if (choice.reasoningContent) message.reasoning_content = choice.reasoningContent;
      if (choice.toolCalls.size > 0) {
        message.tool_calls = [...choice.toolCalls.values()]
          .sort((a, b) => a.index - b.index)
          .map(({ index: _index, ...toolCall }) => toolCall);
      }
      const output: JsonRecord = {
        index: choice.index,
        message,
        finish_reason: choice.finishReason ?? null,
      };
      if (choice.nativeFinishReason !== undefined) {
        output.native_finish_reason = choice.nativeFinishReason;
      }
      return output;
    });

  const usagePayload = [...payloads].reverse().find((payload) => isRecord(payload.usage));
  if (usagePayload) result.usage = usagePayload.usage;
  copyDefined(result, last, ['service_tier', 'cost']);
  return result;
}

function appendOpenAiToolCalls(toolCalls: Map<number, StreamToolCall>, rawToolCalls: unknown) {
  if (!Array.isArray(rawToolCalls)) return;
  for (const rawToolCall of rawToolCalls) {
    if (!isRecord(rawToolCall)) continue;
    const index = typeof rawToolCall.index === 'number' ? rawToolCall.index : 0;
    const toolCall = toolCalls.get(index) ?? {
      index,
      function: { name: '', arguments: '' },
    };
    if (typeof rawToolCall.id === 'string') toolCall.id = rawToolCall.id;
    if (typeof rawToolCall.type === 'string') toolCall.type = rawToolCall.type;
    if (isRecord(rawToolCall.function)) {
      if (typeof rawToolCall.function.name === 'string') {
        toolCall.function.name += rawToolCall.function.name;
      }
      if (typeof rawToolCall.function.arguments === 'string') {
        toolCall.function.arguments += rawToolCall.function.arguments;
      }
    }
    toolCalls.set(index, toolCall);
  }
}

function aggregateOpenAiResponses(payloads: JsonRecord[]): JsonRecord | null {
  const responsePayloads = payloads.filter((payload) => (
    typeof payload.type === 'string' && payload.type.startsWith('response.')
  ));
  if (responsePayloads.length === 0) return null;

  const completed = [...responsePayloads].reverse().find((payload) => (
    payload.type === 'response.completed' && isRecord(payload.response)
  ));
  if (completed && isRecord(completed.response)) return completed.response;

  let content = '';
  let reasoning = '';
  let model: string | undefined;
  let usage: unknown;
  for (const payload of responsePayloads) {
    if (payload.type === 'response.output_text.delta' && typeof payload.delta === 'string') {
      content += payload.delta;
    }
    if ((payload.type === 'response.reasoning_summary_text.delta'
      || payload.type === 'response.reasoning_text.delta')
      && typeof payload.delta === 'string') {
      reasoning += payload.delta;
    }
    if (isRecord(payload.response)) {
      if (typeof payload.response.model === 'string') model = payload.response.model;
      if (isRecord(payload.response.usage)) usage = payload.response.usage;
    }
  }

  const result: JsonRecord = {
    object: 'response',
    status: 'completed',
    output: [{
      type: 'message',
      role: 'assistant',
      content: [{ type: 'output_text', text: content }],
    }],
  };
  if (model) result.model = model;
  if (reasoning) result.reasoning = reasoning;
  if (usage) result.usage = usage;
  return result;
}

function aggregateAnthropic(payloads: JsonRecord[]): JsonRecord | null {
  if (!payloads.some((payload) => payload.type === 'message_start'
    || payload.type === 'content_block_delta'
    || payload.type === 'message_stop')) {
    return null;
  }

  const start = payloads.find((payload) => payload.type === 'message_start'
    && isRecord(payload.message));
  const message = start && isRecord(start.message) ? start.message : {};
  let text = '';
  let thinking = '';
  let stopReason: unknown;
  let usage = isRecord(message.usage) ? { ...message.usage } : {};

  for (const payload of payloads) {
    const delta = isRecord(payload.delta) ? payload.delta : {};
    if (payload.type === 'content_block_delta') {
      if (delta.type === 'text_delta' && typeof delta.text === 'string') text += delta.text;
      if (delta.type === 'thinking_delta' && typeof delta.thinking === 'string') {
        thinking += delta.thinking;
      }
    }
    if (payload.type === 'message_delta') {
      if (delta.stop_reason != null) stopReason = delta.stop_reason;
      if (isRecord(payload.usage)) usage = { ...usage, ...payload.usage };
    }
  }

  const result: JsonRecord = {};
  copyDefined(result, message, ['id', 'model']);
  result.type = 'message';
  result.role = typeof message.role === 'string' ? message.role : 'assistant';
  result.content = [{ type: 'text', text }];
  if (thinking) (result.content as JsonRecord[]).unshift({ type: 'thinking', thinking });
  result.stop_reason = stopReason ?? null;
  if (Object.keys(usage).length > 0) result.usage = usage;
  return result;
}

function aggregateGemini(payloads: JsonRecord[]): JsonRecord | null {
  const geminiPayloads = payloads.filter((payload) => Array.isArray(payload.candidates));
  if (geminiPayloads.length === 0) return null;

  let text = '';
  let role = 'model';
  let finishReason: unknown;
  let usageMetadata: unknown;
  let modelVersion: unknown;
  const parts: JsonRecord[] = [];

  for (const payload of geminiPayloads) {
    const candidate = (payload.candidates as unknown[]).find(isRecord);
    if (candidate && isRecord(candidate.content)) {
      if (typeof candidate.content.role === 'string') role = candidate.content.role;
      if (Array.isArray(candidate.content.parts)) {
        for (const part of candidate.content.parts) {
          if (isRecord(part) && typeof part.text === 'string') {
            text += part.text;
            const isThought = part.thought === true;
            const last = parts[parts.length - 1];
            if (last && (last.thought === true) === isThought && typeof last.text === 'string') {
              last.text += part.text;
            } else {
              parts.push(isThought ? { thought: true, text: part.text } : { text: part.text });
            }
          }
        }
      }
    }
    if (candidate?.finishReason != null) finishReason = candidate.finishReason;
    if (isRecord(payload.usageMetadata)) usageMetadata = payload.usageMetadata;
    if (payload.modelVersion != null) modelVersion = payload.modelVersion;
  }

  const result: JsonRecord = {
    candidates: [{
      content: {
        role,
        parts: parts.length ? parts : [{ text }],
      },
      finishReason: finishReason ?? null,
    }],
  };
  if (usageMetadata) result.usageMetadata = usageMetadata;
  if (modelVersion) result.modelVersion = modelVersion;
  return result;
}

function copyDefined(target: JsonRecord, source: JsonRecord, keys: string[]) {
  for (const key of keys) {
    if (source[key] !== undefined) target[key] = source[key];
  }
}



