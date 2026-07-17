import { useEffect, useState } from 'react';
import { BrainCircuit, ChevronDown } from 'lucide-react';

import { useI18n } from '../../i18n';
import { MarkdownContent } from './MarkdownContent';

type ReasoningDisclosureProps = {
  content: string;
  isStreaming: boolean;
};

export function ReasoningDisclosure({
  content,
  isStreaming,
}: ReasoningDisclosureProps) {
  const { t } = useI18n();
  const [isOpen, setIsOpen] = useState(isStreaming);

  useEffect(() => {
    setIsOpen(isStreaming);
  }, [isStreaming]);

  return (
    <section
      className={`message-reasoning${isOpen ? ' is-open' : ''}${isStreaming ? ' is-streaming' : ''}`}
    >
      <button
        className="message-reasoning__toggle"
        type="button"
        aria-expanded={isOpen}
        onClick={() => setIsOpen((open) => !open)}
      >
        <span className="message-reasoning__identity">
          <BrainCircuit size={15} aria-hidden="true" />
          <span>{t(isStreaming ? 'chat.thinkingInProgress' : 'chat.thoughtProcess')}</span>
          {isStreaming ? <span className="message-reasoning__pulse" aria-hidden="true" /> : null}
        </span>
        <ChevronDown className="message-reasoning__chevron" size={15} aria-hidden="true" />
      </button>
      <div className="message-reasoning__region" hidden={!isOpen}>
        <div className="message-reasoning__content message-markdown">
          <MarkdownContent content={content} />
        </div>
      </div>
    </section>
  );
}
