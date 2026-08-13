import type { Components } from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import remarkCjkFriendly from 'remark-cjk-friendly/parseOnly';
import remarkGfm from 'remark-gfm';

import { MarkdownCodeBlock } from './MarkdownCodeBlock';
import { MarkdownImage, markdownUrlTransform } from './MarkdownImage';
import { MarkdownLink } from './MarkdownLink';
import { MarkdownTable } from './MarkdownTable';

export const MARKDOWN_COMPONENTS: Components = {
  a: ({ node: _node, ...props }) => <MarkdownLink {...props} />,
  pre: MarkdownCodeBlock,
  img: ({ node: _node, ...props }) => <MarkdownImage {...props} />,
  table: ({ node: _node, ...props }) => <MarkdownTable {...props} />,
};

export const MARKDOWN_REMARK_PLUGINS = [remarkGfm, remarkCjkFriendly];
export const MARKDOWN_REHYPE_PLUGINS = [rehypeHighlight];
export { markdownUrlTransform };
