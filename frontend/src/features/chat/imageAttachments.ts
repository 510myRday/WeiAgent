import type { ImageAttachment } from '../../types';
import { apiAuthorizationHeaders, apiAuthorizationToken } from '../../apiAuth';

export const MAX_IMAGE_COUNT = 8;
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
export const MAX_IMAGE_TOTAL_BYTES = 32 * 1024 * 1024;

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
  if (attachment.source_type === 'base64') {
    return `data:${attachment.media_type};base64,${attachment.data}`;
  }
  if (attachment.source_type === 'local') {
    const token = apiAuthorizationToken();
    const tokenQuery = token ? `&token=${encodeURIComponent(token)}` : '';
    return `/api/assets/image?path=${encodeURIComponent(attachment.path)}${tokenQuery}`;
  }
  try {
    const parsed = new URL(attachment.url, window.location.origin);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return '';
    return parsed.href;
  } catch {
    return '';
  }
}

export async function materializeImageAttachment(
  attachment: ImageAttachment,
): Promise<ImageAttachment> {
  if (attachment.source_type !== 'local') return attachment;

  const imageUrl = attachmentImageUrl(attachment);
  if (!imageUrl) throw new Error('Generated image URL is unavailable');

  const response = await fetch(imageUrl, {
    credentials: 'same-origin',
    headers: apiAuthorizationHeaders(),
  });
  if (!response.ok) {
    throw new Error(`Generated image request failed (${response.status})`);
  }

  const bytes = new Uint8Array(await response.arrayBuffer());
  let binary = '';
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }

  return {
    id: attachment.id,
    name: attachment.name,
    media_type: attachment.media_type,
    is_mask: attachment.is_mask,
    source_type: 'base64',
    data: btoa(binary),
  };
}

export function attachmentByteLength(attachment: ImageAttachment): number {
  if (attachment.source_type !== 'base64') return 0;
  const padding = attachment.data.endsWith('==') ? 2 : attachment.data.endsWith('=') ? 1 : 0;
  return Math.max(0, Math.floor((attachment.data.length * 3) / 4) - padding);
}

export function attachmentTotalBytes(attachments: ImageAttachment[]): number {
  return attachments.reduce((total, attachment) => total + attachmentByteLength(attachment), 0);
}

export function toggleAttachmentMask(
  attachments: ImageAttachment[],
  attachmentId: string,
): ImageAttachment[] {
  return attachments.map((attachment) => ({
    ...attachment,
    is_mask: attachment.id === attachmentId ? !attachment.is_mask : false,
  }));
}

export function removeAttachment(
  attachments: ImageAttachment[],
  attachmentId: string,
): ImageAttachment[] {
  const remaining = attachments.filter((attachment) => attachment.id !== attachmentId);
  if (remaining.some((attachment) => !attachment.is_mask)) return remaining;
  return remaining.map((attachment) => ({ ...attachment, is_mask: false }));
}

export function createAttachmentId(): string {
  return typeof crypto !== 'undefined' && crypto.randomUUID
    ? crypto.randomUUID()
    : `image-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
