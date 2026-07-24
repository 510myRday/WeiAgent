import { lazy, memo, Suspense, useMemo } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import remarkCjkFriendly from 'remark-cjk-friendly/parseOnly';
import remarkGfm from 'remark-gfm';

import { MarkdownCodeBlock } from './MarkdownCodeBlock';
import { MarkdownImage, markdownUrlTransform } from './MarkdownImage';
import { MarkdownLink } from './MarkdownLink';
import { MarkdownTable } from './MarkdownTable';
import {
  containsMathMarkdown,
  normalizeLooseStrongMarkdown,
  normalizeMathMarkdown,
} from './markdownFormatting';

const MathMarkdownContent = lazy(() => import('./MathMarkdownContent'));

type MarkdownContentProps = {
  content: string;
};

const MARKDOWN_COMPONENTS: Components = {
  a: ({ node: _node, ...props }) => <MarkdownLink {...props} />,
  pre: MarkdownCodeBlock,
  img: ({ node: _node, ...props }) => <MarkdownImage {...props} />,
  table: ({ node: _node, ...props }) => <MarkdownTable {...props} />,
};

const REMARK_PLUGINS = [remarkGfm, remarkCjkFriendly];
const REHYPE_PLUGINS = [rehypeHighlight];

function PlainMarkdownContent({ content }: MarkdownContentProps) {
  return (
    <ReactMarkdown
      remarkPlugins={REMARK_PLUGINS}
      rehypePlugins={REHYPE_PLUGINS}
      components={MARKDOWN_COMPONENTS}
      urlTransform={markdownUrlTransform}
    >
      {content}
    </ReactMarkdown>
  );
}

export const MarkdownContent = memo(function MarkdownContent({ content }: MarkdownContentProps) {
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
