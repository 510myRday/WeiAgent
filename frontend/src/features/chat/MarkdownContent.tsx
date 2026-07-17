import { lazy, memo, Suspense, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import remarkCjkFriendly from 'remark-cjk-friendly/parseOnly';
import remarkGfm from 'remark-gfm';

import { MarkdownCodeBlock } from './MarkdownCodeBlock';
import {
  containsMathMarkdown,
  normalizeLooseStrongMarkdown,
  normalizeMathMarkdown,
} from './markdownFormatting';

const MathMarkdownContent = lazy(() => import('./MathMarkdownContent'));

function PlainMarkdownContent({ content }: { content: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm, remarkCjkFriendly]}
      rehypePlugins={[rehypeHighlight]}
      components={{ pre: MarkdownCodeBlock }}
    >
      {content}
    </ReactMarkdown>
  );
}

export const MarkdownContent = memo(function MarkdownContent({ content }: { content: string }) {
  const normalized = useMemo(
    () => normalizeMathMarkdown(normalizeLooseStrongMarkdown(content)),
    [content],
  );
  if (!containsMathMarkdown(content)) {
    return <PlainMarkdownContent content={normalized} />;
  }
  return (
    <Suspense fallback={<PlainMarkdownContent content={normalized} />}>
      <MathMarkdownContent content={normalized} />
    </Suspense>
  );
});
