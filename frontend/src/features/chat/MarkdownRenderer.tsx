import { lazy, Suspense } from 'react';
import ReactMarkdown from 'react-markdown';
import 'highlight.js/styles/github.css';

import { containsMathMarkdown } from './markdownFormatting';
import {
  MARKDOWN_COMPONENTS,
  MARKDOWN_REHYPE_PLUGINS,
  MARKDOWN_REMARK_PLUGINS,
  markdownUrlTransform,
} from './markdownConfig';

const MathMarkdownContent = lazy(() => import('./MathMarkdownContent'));

type MarkdownRendererProps = {
  content: string;
};

function PlainMarkdownContent({ content }: MarkdownRendererProps) {
  return (
    <ReactMarkdown
      remarkPlugins={MARKDOWN_REMARK_PLUGINS}
      rehypePlugins={MARKDOWN_REHYPE_PLUGINS}
      components={MARKDOWN_COMPONENTS}
      urlTransform={markdownUrlTransform}
    >
      {content}
    </ReactMarkdown>
  );
}

export default function MarkdownRenderer({ content }: MarkdownRendererProps) {
  if (!containsMathMarkdown(content)) {
    return <PlainMarkdownContent content={content} />;
  }

  return (
    <Suspense fallback={<PlainMarkdownContent content={content} />}>
      <MathMarkdownContent content={content} />
    </Suspense>
  );
}
