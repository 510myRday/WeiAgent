import { useState, type ImgHTMLAttributes } from 'react';
import { ImageOff } from 'lucide-react';
import { defaultUrlTransform, type UrlTransform } from 'react-markdown';

import { useI18n } from '../../i18n';

type MarkdownImageProps = ImgHTMLAttributes<HTMLImageElement>;

export function MarkdownImage({ alt = '', src = '', title }: MarkdownImageProps) {
  const { t } = useI18n();
  const [failed, setFailed] = useState(false);
  const resolved = resolveMarkdownImageSource(src);

  if (!resolved || failed) {
    return (
      <span className="markdown-image-error" role="img" aria-label={t('chat.imageLoadFailed')}>
        <ImageOff size={18} aria-hidden="true" />
        <span>{alt || t('chat.imageLoadFailed')}</span>
      </span>
    );
  }

  return (
    <a
      className="markdown-image-link"
      href={resolved.url}
      target="_blank"
      rel="noopener noreferrer"
      title={title || t('chat.openImage')}
    >
      <img
        src={resolved.url}
        alt={alt}
        title={title}
        loading="lazy"
        decoding="async"
        referrerPolicy={resolved.isLocal ? undefined : 'no-referrer'}
        onError={() => setFailed(true)}
      />
    </a>
  );
}

export const markdownUrlTransform: UrlTransform = (url, key, node) => {
  if (
    key === 'src'
    && node.tagName === 'img'
    && (isLocalImageSource(url) || isInlineImageSource(url))
  ) {
    return url;
  }
  return defaultUrlTransform(url);
};

export function resolveMarkdownImageSource(
  source: string,
): { url: string; isLocal: boolean } | null {
  if (!source) return null;
  if (!isLocalImageSource(source)) {
    return { url: source, isLocal: false };
  }

  const path = localPathFromSource(source);
  if (!path) return null;
  const query = new URLSearchParams({ path });
  return { url: `/api/assets/local?${query.toString()}`, isLocal: true };
}

export function isLocalImageSource(source: string): boolean {
  const decoded = decodeSource(source);
  return (
    /^file:\/\//i.test(decoded)
    || /^[a-z]:[\\/]/i.test(decoded)
    || /^\\\\/.test(decoded)
    || /^\/(?!\/)/.test(decoded)
  );
}

function isInlineImageSource(source: string): boolean {
  return /^data:image\/(?:png|jpe?g|gif|webp|avif|bmp);base64,/i.test(source);
}

function localPathFromSource(source: string): string {
  const decoded = decodeSource(source);
  if (!/^file:\/\//i.test(decoded)) return decoded;

  try {
    const url = new URL(decoded);
    const pathname = decodeSource(url.pathname);
    if (url.hostname) {
      return `\\\\${url.hostname}${pathname.replace(/\//g, '\\')}`;
    }
    return /^\/[a-z]:\//i.test(pathname) ? pathname.slice(1) : pathname;
  } catch {
    return decoded.replace(/^file:\/\//i, '');
  }
}

function decodeSource(source: string): string {
  try {
    return decodeURIComponent(source);
  } catch {
    return source;
  }
}
