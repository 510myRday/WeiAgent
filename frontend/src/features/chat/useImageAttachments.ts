import {
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
} from 'react';

import type { ImageAttachment } from '../../types';
import { useFeedback } from '../../components/ui';
import { useI18n } from '../../i18n';
import {
  MAX_IMAGE_BYTES,
  MAX_IMAGE_COUNT,
  MAX_IMAGE_TOTAL_BYTES,
  attachmentTotalBytes,
  createAttachmentId,
  fileToBase64,
  isFileDrag,
  normalizedImageType,
} from './imageAttachments';

interface UseImageAttachmentsOptions {
  supportsImages: boolean;
  /** When true, drag/drop interactions are ignored (e.g. disabled or sending). */
  interactionDisabled: boolean;
  /** Defaults to a toast warning when images are not supported by the model. */
  onUnsupportedImage?: () => void;
}

/**
 * Shared image-attachment state + paste/drag/drop handlers for the composer
 * and the message edit form.
 */
export function useImageAttachments({
  supportsImages,
  interactionDisabled,
  onUnsupportedImage,
}: UseImageAttachmentsOptions) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const [attachments, setAttachments] = useState<ImageAttachment[]>([]);
  const [isDraggingFiles, setIsDraggingFiles] = useState(false);
  const dragDepthRef = useRef(0);

  function resetDragState() {
    dragDepthRef.current = 0;
    setIsDraggingFiles(false);
  }

  async function addFiles(files: File[]) {
    if (files.length === 0) return;
    if (!supportsImages) {
      if (onUnsupportedImage) {
        onUnsupportedImage();
      } else {
        toast.warning(t('chat.imageUnsupportedModel'));
      }
      return;
    }
    const remaining = MAX_IMAGE_COUNT - attachments.length;
    if (remaining <= 0) {
      toast.warning(t('chat.imageCountLimit', { count: MAX_IMAGE_COUNT }));
      return;
    }
    const accepted: ImageAttachment[] = [];
    let acceptedBytes = 0;
    const currentBytes = attachmentTotalBytes(attachments);
    let totalLimitReached = false;
    for (const file of files.slice(0, remaining)) {
      const mediaType = normalizedImageType(file);
      if (!mediaType) {
        toast.warning(t('chat.imageTypeUnsupported'));
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        toast.warning(t('chat.imageTooLarge', { size: 10 }));
        continue;
      }
      if (currentBytes + acceptedBytes + file.size > MAX_IMAGE_TOTAL_BYTES) {
        totalLimitReached = true;
        continue;
      }
      accepted.push({
        id: createAttachmentId(),
        name: file.name || t('chat.pastedImage'),
        media_type: mediaType,
        source_type: 'base64',
        data: await fileToBase64(file),
      });
      acceptedBytes += file.size;
    }
    if (files.length > remaining) {
      toast.warning(t('chat.imageCountLimit', { count: MAX_IMAGE_COUNT }));
    }
    if (totalLimitReached) {
      toast.warning(t('chat.imageTotalTooLarge', { size: 32 }));
    }
    if (accepted.length > 0) {
      setAttachments((current) => [...current, ...accepted]);
    }
  }

  function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const imageFiles = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === 'file' && item.type.startsWith('image/'))
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null);
    if (imageFiles.length === 0) return;
    event.preventDefault();
    void addFiles(imageFiles);
  }

  function handleDragEnter(event: DragEvent<HTMLElement>) {
    if (interactionDisabled || !isFileDrag(event.dataTransfer)) return;
    event.preventDefault();
    event.stopPropagation();
    dragDepthRef.current += 1;
    setIsDraggingFiles(true);
  }

  function handleDragOver(event: DragEvent<HTMLElement>) {
    if (interactionDisabled || !isFileDrag(event.dataTransfer)) return;
    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = 'copy';
  }

  function handleDragLeave(event: DragEvent<HTMLElement>) {
    if (!isFileDrag(event.dataTransfer)) return;
    event.preventDefault();
    event.stopPropagation();
    dragDepthRef.current = Math.max(0, dragDepthRef.current - 1);
    if (dragDepthRef.current === 0) setIsDraggingFiles(false);
  }

  function handleDrop(event: DragEvent<HTMLElement>) {
    if (!isFileDrag(event.dataTransfer)) return;
    event.preventDefault();
    event.stopPropagation();
    resetDragState();
    if (interactionDisabled) return;
    void addFiles(Array.from(event.dataTransfer.files));
  }

  return {
    attachments,
    setAttachments,
    isDraggingFiles,
    resetDragState,
    addFiles,
    handlePaste,
    handleDragEnter,
    handleDragOver,
    handleDragLeave,
    handleDrop,
  };
}
