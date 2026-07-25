import { memo } from 'react';

import { MarkdownContent } from './MarkdownContent';

/**
 * Byte offset of the last blank line outside fenced code blocks — everything
 * before it is treated as a stable, fully-formed markdown prefix.
 */
function findStableSplit(content: string): number {
  let inFence = false;
  let lastSafe = 0;
  let offset = 0;
  for (const line of content.split('\n')) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith('```') || trimmed.startsWith('~~~')) {
      inFence = !inFence;
    }
    if (!inFence && line.trim() === '') {
      lastSafe = offset;
    }
    offset += line.length + 1;
  }
  return lastSafe;
}

/**
 * Markdown renderer for actively streaming content. The stable prefix only
 * changes when a paragraph completes, so its memoized MarkdownContent skips
 * re-parsing on every delta flush; only the short live tail re-parses.
 * Once the message finishes streaming, callers should switch back to a plain
 * MarkdownContent so the final document renders as a single parse.
 */
export const StreamingMarkdown = memo(function StreamingMarkdown({ content }: { content: string }) {
  const splitIndex = findStableSplit(content);
  const stable = content.slice(0, splitIndex);
  const tail = content.slice(splitIndex);
  return (
    <>
      {stable.trim() ? <MarkdownContent content={stable} /> : null}
      {tail.trim() ? <MarkdownContent content={tail} /> : null}
    </>
  );
});
