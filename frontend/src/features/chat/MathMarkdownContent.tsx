import ReactMarkdown from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import rehypeKatex from 'rehype-katex';
import remarkCjkFriendly from 'remark-cjk-friendly/parseOnly';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import 'katex/dist/katex.min.css';

import { MarkdownCodeBlock } from './MarkdownCodeBlock';

export default function MathMarkdownContent({ content }: { content: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm, remarkCjkFriendly, remarkMath]}
      rehypePlugins={[rehypeKatex, rehypeHighlight]}
      components={{ pre: MarkdownCodeBlock }}
    >
      {content}
    </ReactMarkdown>
  );
}
