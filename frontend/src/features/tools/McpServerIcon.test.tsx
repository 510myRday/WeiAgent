import { fireEvent, render } from '@testing-library/react';
import { expect, test } from 'vitest';

import { McpServerIcon } from './McpServerIcon';

test('MCP server icon falls back to the default icon when the custom image fails', () => {
  const { container } = render(<McpServerIcon logoUrl="https://example.com/icon.png" />);
  const image = container.querySelector('img');

  expect(image).not.toBeNull();
  fireEvent.error(image!);

  expect(container.querySelector('img')).toBeNull();
  expect(container.querySelector('svg')).not.toBeNull();
});
