import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';

import { MessageTimeline } from './MessageTimeline';

const items = [
  { messageIndex: 2, dateLabel: '07/17', timeLabel: '09:00', preview: 'First' },
  { messageIndex: 5, dateLabel: '07/17', timeLabel: '09:05', preview: 'Second' },
  { messageIndex: 9, dateLabel: '07/17', timeLabel: '09:10', preview: 'Third' },
];

test('timeline listbox supports arrows, Home, and End', async () => {
  const user = userEvent.setup();
  const onSelect = vi.fn();
  render(
    <MessageTimeline
      items={items}
      activeMessageIndex={5}
      label="Timeline"
      onSelect={onSelect}
    />,
  );
  const listbox = screen.getByRole('listbox', { name: 'Timeline' });
  listbox.focus();

  await user.keyboard('{ArrowDown}');
  expect(onSelect).toHaveBeenLastCalledWith(9);

  await user.keyboard('{Home}');
  expect(onSelect).toHaveBeenLastCalledWith(2);

  await user.keyboard('{End}');
  expect(onSelect).toHaveBeenLastCalledWith(9);
});
