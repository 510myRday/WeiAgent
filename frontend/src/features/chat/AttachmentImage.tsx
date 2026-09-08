import { useEffect, useState, type ImgHTMLAttributes } from 'react';

import type { ImageAttachment } from '../../types';
import { apiAuthorizationHeaders } from '../../apiAuth';
import { attachmentImageUrl } from './imageAttachments';

type AttachmentImageProps = Omit<ImgHTMLAttributes<HTMLImageElement>, 'src'> & {
  attachment: ImageAttachment;
};

export function AttachmentImage({ attachment, onError, ...props }: AttachmentImageProps) {
  const [displaySource, setDisplaySource] = useState('');
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let objectUrl = '';
    const controller = new AbortController();
    const source = attachmentImageUrl(attachment);
    setDisplaySource(attachment.source_type === 'local' ? '' : source);
    setFailed(false);

    if (attachment.source_type === 'local') {
      void fetch(source, {
        credentials: 'same-origin',
        headers: apiAuthorizationHeaders(),
        signal: controller.signal,
      })
        .then((response) => {
          if (!response.ok) throw new Error(response.statusText);
          return response.blob();
        })
        .then((blob) => {
          objectUrl = URL.createObjectURL(blob);
          setDisplaySource(objectUrl);
        })
        .catch((error: unknown) => {
          if (!(error instanceof DOMException && error.name === 'AbortError')) setFailed(true);
        });
    }

    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [attachment]);

  if (!displaySource || failed) return null;
  return (
    <img
      {...props}
      src={displaySource}
      alt={props.alt ?? ''}
      referrerPolicy="no-referrer"
      onError={(event) => {
        setFailed(true);
        onError?.(event);
      }}
    />
  );
}
