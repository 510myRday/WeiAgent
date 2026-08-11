import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ImageAttachment } from '../../types';
import {
  attachmentByteLength,
  attachmentTotalBytes,
  materializeImageAttachment,
  removeAttachment,
  toggleAttachmentMask,
} from './imageAttachments';

afterEach(() => {
  vi.restoreAllMocks();
});

function attachment(id: string, data: string, isMask = false): ImageAttachment {
  return {
    id,
    name: `${id}.png`,
    media_type: 'image/png',
    is_mask: isMask,
    source_type: 'base64',
    data,
  };
}

describe('image attachment helpers', () => {
  it('calculates decoded Base64 byte totals', () => {
    const first = attachment('first', 'AQID');
    const second = attachment('second', 'BAU=');
    expect(attachmentByteLength(first)).toBe(3);
    expect(attachmentByteLength(second)).toBe(2);
    expect(attachmentTotalBytes([first, second])).toBe(5);
  });

  it('keeps at most one selected mask', () => {
    const attachments = [attachment('source', 'AQID'), attachment('mask', 'BAU=')];
    const selected = toggleAttachmentMask(attachments, 'mask');
    expect(selected.map((item) => item.is_mask)).toEqual([false, true]);

    const moved = toggleAttachmentMask(selected, 'source');
    expect(moved.map((item) => item.is_mask)).toEqual([true, false]);
  });

  it('clears an orphaned mask when its source is removed', () => {
    const remaining = removeAttachment(
      [attachment('source', 'AQID'), attachment('mask', 'BAU=', true)],
      'source',
    );
    expect(remaining).toHaveLength(1);
    expect(remaining[0].is_mask).toBe(false);
  });

  it('converts a local generated image into a Base64 reference', async () => {
    const fetchMock = vi.fn(async () => new Response(
      new Uint8Array([0, 255, 1]),
      { status: 200, headers: { 'content-type': 'image/png' } },
    ));
    vi.stubGlobal('fetch', fetchMock);

    const local: ImageAttachment = {
      id: 'generated',
      name: 'generated.png',
      media_type: 'image/png',
      source_type: 'local',
      path: 'C:\\data\\attachments\\generated.png',
    };

    await expect(materializeImageAttachment(local)).resolves.toMatchObject({
      id: 'generated',
      name: 'generated.png',
      media_type: 'image/png',
      source_type: 'base64',
      data: 'AP8B',
    });
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/api/assets/image?path='),
      expect.objectContaining({
        credentials: 'same-origin',
      }),
    );
  });

  it('keeps uploaded Base64 references unchanged', async () => {
    const uploaded = attachment('uploaded', 'AQID');
    await expect(materializeImageAttachment(uploaded)).resolves.toBe(uploaded);
  });
});
