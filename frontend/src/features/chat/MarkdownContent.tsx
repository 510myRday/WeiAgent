import { lazy, memo, Suspense, useMemo } from 'react';
import { useI18n } from '../../i18n';
import {
  normalizeLooseStrongMarkdown,
  normalizeMathMarkdown,
} from './markdownFormatting';

const MarkdownRenderer = lazy(() => import('./MarkdownRenderer'));

type MarkdownContentProps = {
  content: string;
};

export const MarkdownContent = memo(function MarkdownContent({ content }: MarkdownContentProps) {
  const { t } = useI18n();
  const normalized = useMemo(
    () => normalizeMathMarkdown(normalizeLooseStrongMarkdown(content)),
    [content],
  );
  return (
    <Suspense fallback={<span className="markdown-loading" role="status" aria-label={t('chat.markdownLoading')} />}>
      <MarkdownRenderer content={normalized} />
    </Suspense>
  );
});
