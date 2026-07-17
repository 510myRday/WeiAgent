import { useState } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import axe from 'axe-core';

import { WorkspaceTabs, type WorkspaceField } from './WorkspaceTabs';

const fields: WorkspaceField[] = ['personality', 'memory', 'instructions'];

function Fixture() {
  const [active, setActive] = useState<WorkspaceField>('personality');
  return (
    <>
      <WorkspaceTabs
        fields={fields}
        activeField={active}
        label="Workspace"
        panelId="panel"
        getLabel={(field) => field}
        onChange={setActive}
      />
      <div id="panel" role="tabpanel" aria-labelledby={`agent-workspace-tab-${active}`} />
    </>
  );
}

test('workspace tabs use roving focus and arrow navigation', async () => {
  const user = userEvent.setup();
  render(<Fixture />);
  const personality = screen.getByRole('tab', { name: 'personality' });
  const memory = screen.getByRole('tab', { name: 'memory' });

  personality.focus();
  await user.keyboard('{ArrowRight}');

  expect(memory).toHaveFocus();
  expect(memory).toHaveAttribute('aria-selected', 'true');
  expect(memory).toHaveAttribute('tabindex', '0');
  expect(personality).toHaveAttribute('tabindex', '-1');
  expect(screen.getByRole('tabpanel')).toHaveAttribute('aria-labelledby', memory.id);
});

test('workspace tabs have no axe violations', async () => {
  const { container } = render(<Fixture />);
  const results = await axe.run(container, {
    rules: {
      'color-contrast': { enabled: false },
    },
  });
  expect(results.violations).toEqual([]);
});
