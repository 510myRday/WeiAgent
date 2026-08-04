import { describe, expect, it } from 'vitest';
import { formatLlmResponse, formatRawJson } from './llmLogFormatting';

describe('LLM log formatting', () => {
  it('formats regular JSON responses without treating them as streams', () => {
    const response = formatLlmResponse('{"choices":[{"message":{"content":"hello"}}]}');

    expect(response.isStreaming).toBe(false);
    expect(response.formatted).toBe(`{
  "choices": [
    {
      "message": {
        "content": "hello"
      }
    }
  ]
}`);
  });

  it('aggregates OpenAI chat completion chunks and keeps the raw stream', () => {
    const raw = [
      ': processing',
      '',
      'data: {"id":"chat-1","object":"chat.completion.chunk","model":"model-1","choices":[{"index":0,"delta":{"role":"assistant","content":"你"},"finish_reason":null}]}',
      '',
      'data: {"id":"chat-1","object":"chat.completion.chunk","model":"model-1","choices":[{"index":0,"delta":{"content":"好"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}',
      '',
      'data: [DONE]',
    ].join('\n');

    const response = formatLlmResponse(raw);
    const aggregated = JSON.parse(response.formatted);

    expect(response.isStreaming).toBe(true);
    expect(response.raw).toBe(raw);
    expect(aggregated.object).toBe('chat.completion');
    expect(aggregated.choices[0].message).toEqual({
      role: 'assistant',
      content: '你好',
    });
    expect(aggregated.choices[0].finish_reason).toBe('stop');
    expect(aggregated.usage.total_tokens).toBe(3);
  });

  it('aggregates Anthropic and Gemini text deltas', () => {
    const anthropic = formatLlmResponse([
      'event: message_start',
      'data: {"type":"message_start","message":{"id":"msg-1","model":"claude","role":"assistant","usage":{"input_tokens":4}}}',
      'event: content_block_delta',
      'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}',
      'event: message_delta',
      'data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}',
      'event: message_stop',
      'data: {"type":"message_stop"}',
    ].join('\n'));
    const gemini = formatLlmResponse([
      'data: {"candidates":[{"content":{"role":"model","parts":[{"text":"gem"}]}}]}',
      'data: {"candidates":[{"content":{"role":"model","parts":[{"text":"ini"}]},"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":5}}',
    ].join('\n'));

    expect(JSON.parse(anthropic.formatted).content[0].text).toBe('hello');
    expect(JSON.parse(anthropic.formatted).usage).toEqual({
      input_tokens: 4,
      output_tokens: 1,
    });
    expect(JSON.parse(gemini.formatted).candidates[0].content.parts[0].text).toBe('gemini');
    expect(JSON.parse(gemini.formatted).usageMetadata.totalTokenCount).toBe(5);
  });

  it('leaves non-JSON text untouched', () => {
    expect(formatRawJson('upstream error')).toBe('upstream error');
  });

  it('renders the middle truncation marker in the request/response viewer', () => {
    expect(formatRawJson('HEAD\n[middle truncated]\nTAIL')).toBe(
      'HEAD\n[..., 此处已截断, ...]\nTAIL',
    );
  });

  it('marks truncated streams in both the raw view and aggregated JSON', () => {
    const raw = [
      'data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"HEAD"},"finish_reason":null}]}',
      '[LLM stream log middle truncated]',
      'data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"TAIL"},"finish_reason":"stop"}]}',
    ].join('\n');

    const response = formatLlmResponse(raw);
    const aggregated = JSON.parse(response.formatted);

    expect(response.raw).toContain('[..., 此处已截断, ...]');
    expect(aggregated.truncated).toBe(true);
    expect(aggregated.truncation_notice).toBe('[..., 此处已截断, ...]');
  });
});
