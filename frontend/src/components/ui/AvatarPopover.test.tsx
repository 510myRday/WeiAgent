import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { AvatarPopover } from './AvatarPopover';

test('avatar picker opens without a page backdrop and closes on outside click', async () => {
  const user = userEvent.setup();
  render(
    <div>
      <AvatarPopover
        icon="bot"
        color="#07c160"
        title="Avatar preview"
        pickerTitle="Choose avatar"
        trigger={() => <button type="button">Change avatar</button>}
        onChange={() => {}}
      />
      <button type="button">Outside</button>
    </div>,
  );

  await user.click(screen.getByRole('button', { name: 'Change avatar' }));

  expect(screen.getByRole('dialog', { name: 'Choose avatar' })).toBeVisible();
  expect(document.querySelector('.popover-backdrop')).toBeNull();
  expect(screen.getByRole('button', { name: 'Change avatar' })).toHaveAttribute('aria-expanded', 'true');

  await user.click(screen.getByRole('button', { name: 'Outside' }));

  expect(screen.queryByRole('dialog', { name: 'Choose avatar' })).not.toBeInTheDocument();
});
