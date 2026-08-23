import { describe, expect, test } from 'vitest';

import {
  formatTerminalCommand,
  formatToolCommand,
  formatToolOutput,
  hasToolPayload,
} from './ToolCallCard';

describe('formatTerminalCommand', () => {
  test('prefixes a command with its resolved working directory', () => {
    expect(formatTerminalCommand('pwd', 'D:\\Code_Save\\rust\\WeiAgent')).toBe(
      'D:\\Code_Save\\rust\\WeiAgent> pwd',
    );
  });

  test('keeps commands without a known directory unchanged', () => {
    expect(formatTerminalCommand('pwd')).toBe('pwd');
  });

  test('only prefixes execute_terminal commands', () => {
    expect(formatToolCommand('execute_terminal', 'pwd', 'D:\\workspace')).toBe('D:\\workspace> pwd');
    expect(formatToolCommand('some_tool', 'pwd', 'D:\\workspace')).toBe('pwd');
  });
});

describe('hasToolPayload', () => {
  test('does not render an empty card before streamed arguments arrive', () => {
    expect(hasToolPayload({}, '', 'generating_arguments')).toBe(false);
    expect(hasToolPayload({}, '{"command":"pw', 'generating_arguments')).toBe(true);
  });
});

describe('formatToolOutput', () => {
  test('pretty prints compact JSON objects', () => {
    expect(formatToolOutput('{"result_count":1,"results":[{"title":"Example"}]}')).toBe(
      '{\n  "result_count": 1,\n  "results": [\n    {\n      "title": "Example"\n    }\n  ]\n}',
    );
  });

  test('pretty prints JSON lines independently', () => {
    expect(formatToolOutput('{"status":"running"}\n{"status":"done"}')).toBe(
      '{\n  "status": "running"\n}\n{\n  "status": "done"\n}',
    );
  });

  test('preserves ordinary terminal text', () => {
    const output = 'first line\nsecond line: value';
    expect(formatToolOutput(output)).toBe(output);
  });
});
