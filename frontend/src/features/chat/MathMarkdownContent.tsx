import ReactMarkdown from 'react-markdown';
import rehypeKatex from 'rehype-katex';
import remarkMath from 'remark-math';
import 'katex/dist/katex.min.css';

import {
  MARKDOWN_COMPONENTS,
  MARKDOWN_REHYPE_PLUGINS,
  MARKDOWN_REMARK_PLUGINS,
  markdownUrlTransform,
} from './markdownConfig';

export default function MathMarkdownContent({
  content,
}: {
  content: string;
}) {
  return (
    <ReactMarkdown
      remarkPlugins={[...MARKDOWN_REMARK_PLUGINS, remarkMath]}
      rehypePlugins={[rehypeKatex, ...MARKDOWN_REHYPE_PLUGINS]}
      components={MARKDOWN_COMPONENTS}
      urlTransform={markdownUrlTransform}
    >
      {content}
    </ReactMarkdown>
  );
}
