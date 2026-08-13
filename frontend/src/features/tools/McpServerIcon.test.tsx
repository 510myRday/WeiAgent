import { fireEvent, render } from '@testing-library/react';
import { expect, test } from 'vitest';

import { isLocalAssetSource } from '../../hooks';
import { McpServerIcon } from './McpServerIcon';

test.each([
  ['https://example.com/icon.png', false],
  ['D:\\Code_Save\\rust\\WeiAgent\\icon.png', true],
  ['/Users/me/icon.png', true],
  ['/home/me/icon.png', true],
  ['file:///Users/me/icon.png', true],
] as const)('detects local logo source: %s', (source, expected) => {
  expect(isLocalAssetSource(source)).toBe(expected);
});

test('MCP server icon falls back to the default icon when the custom image fails', () => {
  const { container } = render(<McpServerIcon logoUrl="https://example.com/icon.png" />);
  const image = container.querySelector('img');

  expect(image).not.toBeNull();
  fireEvent.error(image!);

  expect(container.querySelector('img')).toBeNull();
  expect(container.querySelector('svg')).not.toBeNull();
});
