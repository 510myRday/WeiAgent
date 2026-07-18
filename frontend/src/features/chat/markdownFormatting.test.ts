import { describe, expect, test } from 'vitest';

import { normalizeLooseStrongMarkdown } from './markdownFormatting';

describe('normalizeLooseStrongMarkdown', () => {
  test('moves spaces inside loose strong markers to the outside', () => {
    expect(normalizeLooseStrongMarkdown('**最高温 ** ** 33°C**')).toBe('**最高温**   **33°C**');
    expect(normalizeLooseStrongMarkdown('** 约 20mm**')).toBe(' **约 20mm**');
    expect(normalizeLooseStrongMarkdown('**约 20mm **')).toBe('**约 20mm** ');
    expect(normalizeLooseStrongMarkdown('**紫外线指数 ** ** 10 (极高) **')).toBe(
      '**紫外线指数**   **10 (极高)** ',
    );
  });

  test('does not merge adjacent table-cell strong spans across |', () => {
    const line = '| 🌡️ **最高温** | **33°C**（体感可达 **41°C**🔥） |';
    expect(normalizeLooseStrongMarkdown(line)).toBe(line);

    const low = '| 🌡️ **最低温** | **24°C** |';
    expect(normalizeLooseStrongMarkdown(low)).toBe(low);

    const rain = '| ☂️ **总降水量** | 约 **20mm**（午后到傍晚雨势明显） |';
    expect(normalizeLooseStrongMarkdown(rain)).toBe(rain);

    const uv = '| 🌀 **紫外线指数** | **10（极高）** |';
    expect(normalizeLooseStrongMarkdown(uv)).toBe(uv);
  });

  test('keeps already-valid strong spans unchanged', () => {
    expect(normalizeLooseStrongMarkdown('**最高温** **33°C**')).toBe('**最高温** **33°C**');
    expect(normalizeLooseStrongMarkdown('| 💧 **湿度** | 61% ~ 97% |')).toBe(
      '| 💧 **湿度** | 61% ~ 97% |',
    );
  });

  test('handles unicode spaces inside loose strong markers', () => {
    expect(normalizeLooseStrongMarkdown('**最高温\u00a0** **\u00a033°C**')).toBe(
      '**最高温**\u00a0 \u00a0**33°C**',
    );
  });

  test('does not rewrite fenced code blocks', () => {
    const content = '```\n** 最高温 **\n```\n**标签 **';
    expect(normalizeLooseStrongMarkdown(content)).toBe('```\n** 最高温 **\n```\n**标签** ');
  });

  test('does not rewrite inline code', () => {
    expect(normalizeLooseStrongMarkdown('code `** 最高温 **` outside **值 **')).toBe(
      'code `** 最高温 **` outside **值** ',
    );
  });

  test('ignores triple-asterisk emphasis runs', () => {
    expect(normalizeLooseStrongMarkdown('***最高温 *** *** 33°C***')).toBe(
      '***最高温 *** *** 33°C***',
    );
  });
});
