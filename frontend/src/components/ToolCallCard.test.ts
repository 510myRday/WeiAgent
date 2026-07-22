import { describe, expect, test } from 'vitest';

import { formatToolOutput } from './ToolCallCard';

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
