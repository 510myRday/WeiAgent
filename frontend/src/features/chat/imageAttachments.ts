import type { ImageAttachment } from '../../types';

export const MAX_IMAGE_COUNT = 8;
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024;

const IMAGE_TYPES = new Set<ImageAttachment['media_type']>([
  'image/png',
  'image/jpeg',
  'image/webp',
  'image/gif',
]);

export function normalizedImageType(file: File): ImageAttachment['media_type'] | null {
  if (IMAGE_TYPES.has(file.type as ImageAttachment['media_type'])) {
    return file.type as ImageAttachment['media_type'];
  }
  const extension = file.name.split('.').pop()?.toLowerCase();
  if (extension === 'png') return 'image/png';
  if (extension === 'jpg' || extension === 'jpeg') return 'image/jpeg';
  if (extension === 'webp') return 'image/webp';
  if (extension === 'gif') return 'image/gif';
  return null;
}

export function isFileDrag(dataTransfer: DataTransfer): boolean {
  return Array.from(dataTransfer.types).includes('Files');
}

export function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('Failed to read image'));
    reader.onload = () => {
      const result = String(reader.result ?? '');
      resolve(result.slice(result.indexOf(',') + 1));
    };
    reader.readAsDataURL(file);
  });
}

export function attachmentImageUrl(attachment: ImageAttachment): string {
  return attachment.source_type === 'base64'
    ? `data:${attachment.media_type};base64,${attachment.data}`
    : attachment.url;
}

export function createAttachmentId(): string {
  return typeof crypto !== 'undefined' && crypto.randomUUID
    ? crypto.randomUUID()
    : `image-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
