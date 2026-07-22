import type { AnchorHTMLAttributes } from 'react';

type MarkdownLinkProps = AnchorHTMLAttributes<HTMLAnchorElement>;

export function MarkdownLink({ children, ...props }: MarkdownLinkProps) {
  return (
    <a {...props} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}
