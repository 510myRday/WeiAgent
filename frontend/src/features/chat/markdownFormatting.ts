export function normalizeLooseStrongMarkdown(content: string): string {
  const parts = content.split(/(\r\n|\r|\n)/);
  let fence: { marker: string; length: number } | null = null;

  for (let index = 0; index < parts.length; index += 2) {
    const line = parts[index];
    const fenceMatch = line.match(/^(?: {0,3}>[ \t]?)* {0,3}(`{3,}|~{3,})/);

    if (fence) {
      if (
        fenceMatch
        && fenceMatch[1][0] === fence.marker
        && fenceMatch[1].length >= fence.length
        && line.slice(fenceMatch[0].length).trim() === ''
      ) {
        fence = null;
      }
      continue;
    }

    if (fenceMatch) {
      fence = { marker: fenceMatch[1][0], length: fenceMatch[1].length };
      continue;
    }

    parts[index] = normalizeInlineStrongSpacing(line);
  }

  return parts.join('');
}

function normalizeInlineStrongSpacing(line: string): string {
  let result = '';
  let plainStart = 0;
  let cursor = 0;

  while (cursor < line.length) {
    if (line[cursor] !== '`' || isEscapedMarkdownCharacter(line, cursor)) {
      cursor += 1;
      continue;
    }

    const openingStart = cursor;
    while (line[cursor] === '`') cursor += 1;
    const markerLength = cursor - openingStart;
    const closingEnd = findClosingBacktickRun(line, cursor, markerLength);

    if (closingEnd === -1) continue;

    result += normalizeStrongSpacing(line.slice(plainStart, openingStart));
    result += line.slice(openingStart, closingEnd);
    plainStart = closingEnd;
    cursor = closingEnd;
  }

  return result + normalizeStrongSpacing(line.slice(plainStart));
}

function findClosingBacktickRun(line: string, start: number, markerLength: number): number {
  let cursor = start;

  while (cursor < line.length) {
    if (line[cursor] !== '`') {
      cursor += 1;
      continue;
    }

    const runStart = cursor;
    while (line[cursor] === '`') cursor += 1;
    if (cursor - runStart === markerLength) return cursor;
  }

  return -1;
}

function isEscapedMarkdownCharacter(value: string, index: number): boolean {
  let slashCount = 0;
  for (let cursor = index - 1; cursor >= 0 && value[cursor] === '\\'; cursor -= 1) {
    slashCount += 1;
  }
  return slashCount % 2 === 1;
}

function normalizeStrongSpacing(value: string): string {
  const markers: number[] = [];

  for (let index = 0; index < value.length - 1; index += 1) {
    if (value[index] !== '*' || value[index + 1] !== '*') continue;
    if (isEscapedMarkdownCharacter(value, index)) continue;
    if (index > 0 && value[index - 1] === '*' && !isEscapedMarkdownCharacter(value, index - 1)) {
      continue;
    }
    if (index + 2 < value.length && value[index + 2] === '*') {
      continue;
    }
    markers.push(index);
    index += 1;
  }

  if (markers.length < 2) return value;

  let result = '';
  let lastIndex = 0;

  for (let markerIndex = 0; markerIndex + 1 < markers.length; markerIndex += 2) {
    const open = markers[markerIndex];
    const close = markers[markerIndex + 1];
    const inner = value.slice(open + 2, close);
    const core = inner.trim();

    if (!core) {
      result += value.slice(lastIndex, close + 2);
      lastIndex = close + 2;
      continue;
    }

    const leadingLength = inner.length - inner.trimStart().length;
    const trailingLength = inner.length - inner.trimEnd().length;
    const leading = inner.slice(0, leadingLength);
    const trailing = trailingLength > 0 ? inner.slice(inner.length - trailingLength) : '';

    result += value.slice(lastIndex, open) + leading + '**' + core + '**' + trailing;
    lastIndex = close + 2;
  }

  return result + value.slice(lastIndex);
}

export function normalizeMathMarkdown(content: string): string {
  return content.replace(/\\\[([\s\S]*?)\\\]/g, (_match, formula: string) => {
    return `$$\n${formula.trim()}\n$$`;
  }).replace(/\\\(([\s\S]*?)\\\)/g, (_match, formula: string) => `$${formula.trim()}$`);
}

export function containsMathMarkdown(content: string): boolean {
  const withoutFences = content.replace(/(^|\n)[ \t]*(`{3,}|~{3,})[\s\S]*?\n[ \t]*\2(?=\n|$)/g, '');
  return /\\\(|\\\[|(^|[^\\])\$\$[\s\S]+?\$\$|(^|[^\\$])\$[^$\n]+\$/m.test(withoutFences);
}
