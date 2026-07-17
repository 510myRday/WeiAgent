import React from 'react';
import ReactDOM from 'react-dom/client';
import { I18nProvider } from './i18n';
import { App } from './App';
import { FeedbackProvider } from './components/ui';
import { AppErrorBoundary } from './components/AppErrorBoundary';
import './styles/index.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <I18nProvider>
      <AppErrorBoundary scope="app">
        <FeedbackProvider>
          <App />
        </FeedbackProvider>
      </AppErrorBoundary>
    </I18nProvider>
  </React.StrictMode>,
);
