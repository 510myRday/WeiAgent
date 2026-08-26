import { memo, useMemo } from 'react';
import {
  normalizeLooseStrongMarkdown,
  normalizeMathMarkdown,
} from './markdownFormatting';
import MarkdownRenderer from './MarkdownRenderer';

type MarkdownContentProps = {
  content: string;
};

export const MarkdownContent = memo(function MarkdownContent({ content }: MarkdownContentProps) {
  const normalized = useMemo(
    () => normalizeMathMarkdown(normalizeLooseStrongMarkdown(content)),
    [content],
  );
  return <MarkdownRenderer content={normalized} />;
});
