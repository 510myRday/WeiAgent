import { isValidElement, type ReactNode } from 'react';
import { Check, Clipboard } from 'lucide-react';

import { useClipboard } from '../../hooks';
import { useI18n } from '../../i18n';

export function MarkdownCodeBlock({ children }: { children?: ReactNode }) {
  const { t } = useI18n();
  const { copied, copy } = useClipboard(1200);
  const text = extractText(children);
  const language = extractLanguage(children);

  return (
    <div className="code-block-wrap">
      <span className="code-language-label">{language}</span>
      <button className="code-copy-button" type="button" onClick={() => void copy(text)} disabled={!text} aria-label={t('chat.copyCode')}>
        {copied ? <Check size={14} /> : <Clipboard size={14} />}
        <span>{copied ? t('chat.copied') : t('chat.copy')}</span>
      </button>
      <pre>{children}</pre>
    </div>
  );
}

function extractLanguage(node: ReactNode): string {
  const className = findCodeClassName(node);
  const raw = className?.match(/language-([\w-]+)/)?.[1] ?? 'text';
  return formatLanguage(raw);
}

function findCodeClassName(node: ReactNode): string | undefined {
  if (Array.isArray(node)) {
    return node.map(findCodeClassName).find(Boolean);
  }
  if (!isValidElement<{ className?: string; children?: ReactNode }>(node)) {
    return undefined;
  }
  return node.props.className ?? findCodeClassName(node.props.children);
}

function formatLanguage(language: string): string {
  const normalized = language.toLowerCase();
  const labels: Record<string, string> = {
    js: 'JS',
    javascript: 'JS',
    jsx: 'JSX',
    ts: 'TS',
    typescript: 'TS',
    tsx: 'TSX',
    html: 'HTML',
    css: 'CSS',
    json: 'JSON',
    md: 'MD',
    markdown: 'MD',
    py: 'Python',
    python: 'Python',
    rs: 'Rust',
    rust: 'Rust',
    sh: 'Shell',
    bash: 'Shell',
    powershell: 'PowerShell',
    ps1: 'PowerShell',
    text: 'Text',
    txt: 'Text',
  };
  return labels[normalized] ?? normalized.toUpperCase();
}

function extractText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(extractText).join('');
  if (isValidElement<{ children?: ReactNode }>(node)) return extractText(node.props.children);
  return '';
}
