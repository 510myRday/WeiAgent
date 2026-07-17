import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { I18nProvider } from '../../i18n';
import { ReasoningDisclosure } from './ReasoningDisclosure';

function renderDisclosure(isStreaming = false) {
  return render(
    <I18nProvider>
      <ReasoningDisclosure
        content="先检查约束，再组织最终答案。"
        isStreaming={isStreaming}
      />
    </I18nProvider>,
  );
}

test('completed reasoning is collapsed by default and can be expanded', async () => {
  const user = userEvent.setup();
  renderDisclosure();

  const toggle = screen.getByRole('button', { name: '思考过程' });
  expect(toggle).toHaveAttribute('aria-expanded', 'false');

  await user.click(toggle);
  expect(toggle).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByText('先检查约束，再组织最终答案。')).toBeVisible();
});

test('streaming reasoning starts expanded', () => {
  renderDisclosure(true);

  expect(screen.getByRole('button', { name: '正在思考' }))
    .toHaveAttribute('aria-expanded', 'true');
});
