import { describe, expect, test } from 'vitest';

import { isLocalImageSource, resolveMarkdownImageSource } from './MarkdownImage';

describe('markdown image sources', () => {
  test('maps encoded Windows paths to the local asset endpoint', () => {
    const resolved = resolveMarkdownImageSource(
      'D:%5CCode_Save%5Crust%5CWeiAgent%5Cdata%5Cmodel_radar.png',
    );

    expect(resolved?.isLocal).toBe(true);
    const url = new URL(resolved!.url, 'http://localhost');
    expect(url.pathname).toBe('/api/assets/local');
    expect(url.searchParams.get('path')).toBe(
      'D:\\Code_Save\\rust\\WeiAgent\\data\\model_radar.png',
    );
    expect(url.searchParams.has('agent_id')).toBe(false);
  });

  test('maps file URLs to local paths', () => {
    const resolved = resolveMarkdownImageSource('file:///D:/charts/result.png');
    const url = new URL(resolved!.url, 'http://localhost');

    expect(url.searchParams.get('path')).toBe('D:/charts/result.png');
  });

  test('keeps remote image URLs unchanged', () => {
    expect(resolveMarkdownImageSource('https://cdn.example.com/chart.png')).toEqual({
      url: 'https://cdn.example.com/chart.png',
      isLocal: false,
    });
    expect(isLocalImageSource('//cdn.example.com/chart.png')).toBe(false);
  });

  test('maps relative workspace image paths to the local asset endpoint', () => {
    const resolved = resolveMarkdownImageSource('attachments/linuxdo.png');
    const url = new URL(resolved!.url, 'http://localhost');

    expect(resolved?.isLocal).toBe(true);
    expect(url.pathname).toBe('/api/assets/local');
    expect(url.searchParams.get('path')).toBe('attachments/linuxdo.png');
  });
});
