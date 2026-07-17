import { useState } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { PermissionModeControl, type PermissionMode } from './PermissionModeControl';

function Fixture() {
  const [value, setValue] = useState<PermissionMode>('request_approval');
  return (
    <PermissionModeControl
      value={value}
      label="Access"
      requestApprovalLabel="Ask"
      requestApprovalDescription="Ask first"
      fullAccessLabel="Always"
      fullAccessDescription="Always allow"
      requestApprovalDisabled={false}
      fullAccessDisabled={false}
      onChange={setValue}
    />
  );
}

test('radio group changes selection and focus with arrow keys', async () => {
  const user = userEvent.setup();
  render(<Fixture />);
  const ask = screen.getByRole('radio', { name: /Ask/ });
  const always = screen.getByRole('radio', { name: /Always/ });

  ask.focus();
  await user.keyboard('{ArrowRight}');

  expect(always).toHaveFocus();
  expect(always).toHaveAttribute('aria-checked', 'true');
  expect(always).toHaveAttribute('tabindex', '0');
  expect(ask).toHaveAttribute('tabindex', '-1');
});
