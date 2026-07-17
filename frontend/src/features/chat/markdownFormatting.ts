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
  return value
    .replace(/(?<![\\*])\*\*(?!\*)((?:(?!\*\*).)*?\S)([ \t]+)\*\*(?!\*)/g, '**$1**$2')
    .replace(/(?<![\\*])\*\*(?!\*)([ \t]+)((?:(?!\*\*).)*?\S)\*\*(?!\*)/g, '$1**$2**');
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
