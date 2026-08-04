import { render, screen, waitFor } from '@testing-library/react';
import { expect, test } from 'vitest';

import { runA11yCheck } from '../../test/a11y';
import { Dialog } from './Dialog';
import { SearchInput } from './SearchInput';

test('SearchInput has an accessible name', async () => {
  const { container } = render(
    <SearchInput value="" onChange={() => undefined} placeholder="Search conversations" />,
  );

  expect(screen.getByRole('textbox', { name: 'Search conversations' })).toBeInTheDocument();
  expect((await runA11yCheck(container)).violations).toHaveLength(0);
});

test('Dialog isolates the background and restores it after closing', async () => {
  const { container, unmount } = render(
    <>
      <button type="button">Background action</button>
      <Dialog open title="Confirm" closeLabel="Close" onClose={() => undefined}>
        <button type="button">Confirm action</button>
      </Dialog>
    </>,
  );

  await waitFor(() => expect(container).toHaveAttribute('inert'));
  expect((await runA11yCheck(document.querySelector('.dialog-backdrop')!)).violations).toHaveLength(0);

  unmount();
  expect(container).not.toHaveAttribute('inert');
});
