import { createContext, useContext, useEffect, useId, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { Code2, Maximize2, Minimize2, X } from 'lucide-react';
import { createPortal } from 'react-dom';

import { useI18n } from '../../i18n';
import { useFeedback } from '../../components/ui';

const MAX_MINIMIZED_PREVIEWS = 5;

type HtmlPreview = { id: number; html: string };
type HtmlPreviewContextValue = { openHtmlPreview: (html: string) => void };
const HtmlPreviewContext = createContext<HtmlPreviewContextValue | null>(null);

export function HtmlPreviewProvider({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const nextIdRef = useRef(1);
  const [activePreview, setActivePreview] = useState<HtmlPreview | null>(null);
  const [minimizedPreviews, setMinimizedPreviews] = useState<HtmlPreview[]>([]);

  function openHtmlPreview(html: string) {
    const nextPreview = { id: nextIdRef.current++, html };
    setActivePreview((current) => {
      if (!current) return nextPreview;
      setMinimizedPreviews((items) => items.length >= MAX_MINIMIZED_PREVIEWS ? items : [...items, current]);
      return nextPreview;
    });
  }

  function minimizeActivePreview() {
    if (!activePreview) return;
    if (minimizedPreviews.length >= MAX_MINIMIZED_PREVIEWS) {
      toast.warning(t('chat.htmlPreviewLimit'));
      return;
    }
    setMinimizedPreviews((items) => [...items, activePreview]);
    setActivePreview(null);
  }

  function restorePreview(preview: HtmlPreview) {
    setMinimizedPreviews((items) => items.filter((item) => item.id !== preview.id));
    setActivePreview((current) => {
      if (current) {
        setMinimizedPreviews((items) => items.length >= MAX_MINIMIZED_PREVIEWS ? items : [...items, current]);
      }
      return preview;
    });
  }

  return (
    <HtmlPreviewContext.Provider value={{ openHtmlPreview }}>
      {children}
      <HtmlPreviewSurface
        activePreview={activePreview}
        minimizedPreviews={minimizedPreviews}
        onMinimize={minimizeActivePreview}
        onRestore={restorePreview}
        onCloseActive={() => setActivePreview(null)}
        onCloseMinimized={(id) => setMinimizedPreviews((items) => items.filter((item) => item.id !== id))}
      />
    </HtmlPreviewContext.Provider>
  );
}

export function useHtmlPreview(): HtmlPreviewContextValue {
  return useContext(HtmlPreviewContext) ?? { openHtmlPreview: () => {} };
}

function HtmlPreviewSurface({
  activePreview,
  minimizedPreviews,
  onMinimize,
  onRestore,
  onCloseActive,
  onCloseMinimized,
}: {
  activePreview: HtmlPreview | null;
  minimizedPreviews: HtmlPreview[];
  onMinimize: () => void;
  onRestore: (preview: HtmlPreview) => void;
  onCloseActive: () => void;
  onCloseMinimized: (id: number) => void;
}) {
  if (!activePreview && minimizedPreviews.length === 0) return null;
  return createPortal(
    <>
      {activePreview ? <ActivePreviewWindow preview={activePreview} onMinimize={onMinimize} onClose={onCloseActive} /> : null}
      {minimizedPreviews.map((preview, index) => (
        <MinimizedPreviewWindow
          key={preview.id}
          preview={preview}
          index={index}
          onRestore={() => onRestore(preview)}
          onClose={() => onCloseMinimized(preview.id)}
        />
      ))}
    </>,
    document.body,
  );
}

function ActivePreviewWindow({ preview, onMinimize, onClose }: { preview: HtmlPreview; onMinimize: () => void; onClose: () => void }) {
  const { t } = useI18n();
  const titleId = useId();
  const windowRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    previousFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    const focusFrame = window.requestAnimationFrame(() => windowRef.current?.focus());
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') { event.preventDefault(); onClose(); }
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener('keydown', handleKeyDown);
      document.body.style.overflow = previousOverflow;
      previousFocusRef.current?.focus();
    };
  }, [onClose]);

  return (
    <div className="html-preview-layer">
      <button type="button" className="html-preview-backdrop" tabIndex={-1} aria-label={t('chat.closeHtmlPreview')} onClick={onClose} />
      <div ref={windowRef} className="html-preview-window" role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
        <PreviewWindowHeader titleId={titleId} actionLabel={t('chat.minimizeHtmlPreview')} onAction={onMinimize} onClose={onClose} />
        <PreviewFrame html={preview.html} />
      </div>
    </div>
  );
}

function MinimizedPreviewWindow({ preview, index, onRestore, onClose }: { preview: HtmlPreview; index: number; onRestore: () => void; onClose: () => void }) {
  const { t } = useI18n();
  return (
    <div className="html-preview-window is-minimized" style={{ '--html-preview-index': index } as CSSProperties} role="dialog" aria-label={t('chat.htmlPreview')}>
      <PreviewWindowHeader actionLabel={t('chat.restoreHtmlPreview')} onAction={onRestore} onClose={onClose} minimized />
      <PreviewFrame html={preview.html} />
    </div>
  );
}

function PreviewWindowHeader({ titleId, actionLabel, onAction, onClose, minimized = false }: { titleId?: string; actionLabel: string; onAction: () => void; onClose: () => void; minimized?: boolean }) {
  const { t } = useI18n();
  return (
    <header className="html-preview-window__header">
      <div className="html-preview-window__title">
        <span className="html-preview-window__icon"><Code2 size={16} /></span>
        <strong id={titleId}>{t('chat.htmlPreview')}</strong>
      </div>
      <div className="html-preview-window__actions">
        <button className="html-preview-window__action" type="button" onClick={onAction} aria-label={actionLabel} title={actionLabel}>
          {minimized ? <Maximize2 size={16} /> : <Minimize2 size={16} />}
        </button>
        <button className="html-preview-window__action" type="button" onClick={onClose} aria-label={t('chat.closeHtmlPreview')} title={t('chat.closeHtmlPreview')}>
          <X size={17} />
        </button>
      </div>
    </header>
  );
}

function PreviewFrame({ html }: { html: string }) {
  const { t } = useI18n();
  return <iframe className="html-preview-frame" title={t('chat.htmlPreviewFrame')} srcDoc={html} sandbox="allow-forms allow-modals allow-popups allow-scripts" referrerPolicy="no-referrer" />;
}
