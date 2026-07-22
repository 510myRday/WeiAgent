import ReactMarkdown, { type Components } from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import rehypeKatex from 'rehype-katex';
import remarkCjkFriendly from 'remark-cjk-friendly/parseOnly';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import 'katex/dist/katex.min.css';

import { MarkdownCodeBlock } from './MarkdownCodeBlock';
import { MarkdownImage, markdownUrlTransform } from './MarkdownImage';
import { MarkdownLink } from './MarkdownLink';
import { MarkdownTable } from './MarkdownTable';

export default function MathMarkdownContent({
  content,
}: {
  content: string;
}) {
  const components: Components = {
    a: ({ node: _node, ...props }) => <MarkdownLink {...props} />,
    pre: MarkdownCodeBlock,
    img: ({ node: _node, ...props }) => <MarkdownImage {...props} />,
    table: ({ node: _node, ...props }) => <MarkdownTable {...props} />,
  };
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm, remarkCjkFriendly, remarkMath]}
      rehypePlugins={[rehypeKatex, rehypeHighlight]}
      components={components}
      urlTransform={markdownUrlTransform}
    >
      {content}
    </ReactMarkdown>
  );
}
