import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import zh from '../locales/zh.json';
import en from '../locales/en.json';

export type Locale = 'zh' | 'en';

const messages: Record<Locale, typeof zh> = { zh, en };

const STORAGE_KEY = 'weiagent-locale';

export function isLocale(value: unknown): value is Locale {
  return value === 'zh' || value === 'en';
}

function detectLocale(): Locale {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === 'zh' || stored === 'en') return stored;
  } catch { /* localStorage 不可用 */ }
  return 'zh';
}

/** 按 "a.b.c" 路径从嵌套对象中取值 */
function getByPath(obj: Record<string, unknown>, path: string): string {
  const keys = path.split('.');
  let current: unknown = obj;
  for (const key of keys) {
    if (current == null || typeof current !== 'object') return path;
    current = (current as Record<string, unknown>)[key];
  }
  return typeof current === 'string' ? current : path;
}

/* ---- Context ---- */

type I18nContextValue = {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  setLocaleStateOnly: (locale: Locale) => void;
  t: (path: string, params?: Record<string, string | number>) => string;
};

const I18nContext = createContext<I18nContextValue | null>(null);

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(detectLocale);

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    try { localStorage.setItem(STORAGE_KEY, next); } catch { /* noop */ }
  }, []);

  const setLocaleStateOnly = useCallback((next: Locale) => {
    setLocaleState(next);
    try { localStorage.setItem(STORAGE_KEY, next); } catch { /* noop */ }
  }, []);

  useEffect(() => {
    document.documentElement.lang = locale === 'zh' ? 'zh-CN' : 'en';
    document.title = locale === 'zh' ? 'Wei Agent · 智能体工作台' : 'Wei Agent · Agent Workspace';
  }, [locale]);

  const t = useCallback(
    (path: string, params?: Record<string, string | number>) => {
      const template = getByPath(messages[locale] as unknown as Record<string, unknown>, path);
      if (!params) return template;
      return template.replace(/\{(\w+)\}/g, (match, key: string) => (
        Object.prototype.hasOwnProperty.call(params, key) ? String(params[key]) : match
      ));
    },
    [locale],
  );

  const value = useMemo(() => ({ locale, setLocale, setLocaleStateOnly, t }), [locale, setLocale, setLocaleStateOnly, t]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error('useI18n must be used within <I18nProvider>');
  return ctx;
}
