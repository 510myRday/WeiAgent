import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { FeedbackProvider } from '../../components/ui';
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

test('reasoning code blocks use the shared markdown styling scope', async () => {
  const user = userEvent.setup();
  const { container } = render(
    <I18nProvider>
      <FeedbackProvider>
        <ReasoningDisclosure
          content={'引用用户原文：\n\n```text\n13. 请问 百度百科是什么意思\n```'}
          isStreaming={false}
        />
      </FeedbackProvider>
    </I18nProvider>,
  );

  await user.click(screen.getByRole('button', { name: '思考过程' }));

  const reasoningContent = container.querySelector('.message-reasoning__content');
  expect(reasoningContent).toHaveClass('message-markdown');
  expect(reasoningContent?.querySelector('pre')).toHaveTextContent('13. 请问 百度百科是什么意思');
  expect(screen.getByText('Text')).toBeVisible();
  expect(screen.getByRole('button', { name: '复制代码' })).toBeEnabled();
});
