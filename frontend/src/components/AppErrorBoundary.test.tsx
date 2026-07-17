import { render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import type { ReactNode } from 'react';

import { I18nProvider } from '../i18n';
import { AppErrorBoundary } from './AppErrorBoundary';

function Broken(): ReactNode {
  throw new Error('broken');
}

test('feature error boundary renders a retryable fallback', () => {
  const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
  const preventExpectedError = (event: ErrorEvent) => event.preventDefault();
  window.addEventListener('error', preventExpectedError);
  render(
    <I18nProvider>
      <AppErrorBoundary scope="feature">
        <Broken />
      </AppErrorBoundary>
    </I18nProvider>,
  );

  expect(screen.getByRole('alert')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument();
  window.removeEventListener('error', preventExpectedError);
  errorSpy.mockRestore();
});
