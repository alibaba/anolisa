import React, {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useState,
} from 'react';

export const SUPPORTED_LOCALES = ['en-US', 'zh-CN'] as const;

export type Locale = (typeof SUPPORTED_LOCALES)[number];

const DEFAULT_LOCALE: Locale = 'en-US';
const LOCALE_STORAGE_KEY = 'agentsight.locale';

const enUSMessages = {
  'app.title': 'Agent Observability',
  'app.loading': 'Loading...',
  'language.label': 'Language',
  'nav.agentHealth': 'Agent Dashboard',
  'nav.agentObservability': 'Agent Observability',
  'nav.sessions': 'Sessions',
  'nav.tokenSavings': 'Token Savings',
  'nav.optimization': 'Optimization',
  'nav.skillMetrics': 'Skill Metrics',
  'nav.securityObservability': 'Security Observability',
  'nav.systemAudit': 'System Audit',
  'nav.riskEnforcement': 'Risk Enforcement',
  'nav.trajectoryViewer': 'Trajectory Viewer',
  'nav.settings': 'Settings',
  'login.subtitle': 'Enter your dashboard token to continue',
  'login.tokenLabel': 'Dashboard Token',
  'login.tokenPlaceholder': 'Paste your token here',
  'login.error.required': 'Please enter a token',
  'login.error.invalid': 'Invalid token. Check the token with `agentsight dashboard`.',
  'login.error.connection': 'Connection error. Is the AgentSight server running?',
  'login.verifying': 'Verifying...',
  'login.signIn': 'Sign In',
  'login.tokenHintPrefix': 'Run ',
  'login.tokenHintSuffix': ' to view your token.',
  'login.fullTokenHintPrefix': 'Or use ',
  'login.fullTokenHintSuffix': ' to show the complete value.',
} as const;

export type MessageKey = keyof typeof enUSMessages;

const messages: Record<Locale, Record<MessageKey, string>> = {
  'en-US': enUSMessages,
  'zh-CN': {
    'app.title': 'Agent可观测',
    'app.loading': '加载中...',
    'language.label': '语言',
    'nav.agentHealth': 'Agent 看板',
    'nav.agentObservability': 'Agent 可观测',
    'nav.sessions': '会话列表',
    'nav.tokenSavings': 'Token 节省',
    'nav.optimization': '优化分析',
    'nav.skillMetrics': 'Skill 指标',
    'nav.securityObservability': '安全可观测',
    'nav.systemAudit': '系统审计',
    'nav.riskEnforcement': '风险拦截',
    'nav.trajectoryViewer': '轨迹查看',
    'nav.settings': '设置',
    'login.subtitle': '请输入 Dashboard 令牌以继续',
    'login.tokenLabel': 'Dashboard 令牌',
    'login.tokenPlaceholder': '在此粘贴令牌',
    'login.error.required': '请输入令牌',
    'login.error.invalid': '令牌无效。请运行 `agentsight dashboard` 检查令牌。',
    'login.error.connection': '连接失败。请确认 AgentSight 服务正在运行。',
    'login.verifying': '验证中...',
    'login.signIn': '登录',
    'login.tokenHintPrefix': '运行 ',
    'login.tokenHintSuffix': ' 查看令牌。',
    'login.fullTokenHintPrefix': '或使用 ',
    'login.fullTokenHintSuffix': ' 显示完整令牌。',
  },
};

interface I18nContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: (key: MessageKey) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

function isSupportedLocale(value: string | null): value is Locale {
  return value === 'en-US' || value === 'zh-CN';
}

function readPersistedLocale(): string | null {
  if (typeof window === 'undefined') return null;

  try {
    return window.localStorage.getItem(LOCALE_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function resolveLocale(
  persistedLocale: string | null,
  browserLanguages: readonly string[],
): Locale {
  if (isSupportedLocale(persistedLocale)) return persistedLocale;

  for (const browserLanguage of browserLanguages) {
    if (!browserLanguage) continue;
    const normalizedLanguage = browserLanguage.toLowerCase();
    if (normalizedLanguage.startsWith('zh')) return 'zh-CN';
    if (normalizedLanguage.startsWith('en')) return 'en-US';
  }

  return DEFAULT_LOCALE;
}

function resolveInitialLocale(): Locale {
  let browserLanguages: readonly string[] = [];
  if (typeof navigator !== 'undefined') {
    browserLanguages = navigator.languages?.length > 0
      ? navigator.languages
      : [navigator.language];
  }

  return resolveLocale(readPersistedLocale(), browserLanguages);
}

function syncDocumentMetadata(locale: Locale): void {
  if (typeof document !== 'undefined') {
    document.documentElement.lang = locale;
    document.title = messages[locale]['app.title'];
  }
}

export const I18nProvider: React.FC<React.PropsWithChildren> = ({ children }) => {
  const [locale, setLocaleState] = useState<Locale>(resolveInitialLocale);

  useLayoutEffect(() => {
    syncDocumentMetadata(locale);
  }, [locale]);

  const setLocale = useCallback((nextLocale: Locale) => {
    setLocaleState(nextLocale);

    try {
      window.localStorage.setItem(LOCALE_STORAGE_KEY, nextLocale);
    } catch {
      // Keep the in-memory selection when storage is unavailable.
    }
  }, []);

  const t = useCallback(
    (key: MessageKey) => messages[locale][key],
    [locale],
  );

  const value = useMemo(
    () => ({ locale, setLocale, t }),
    [locale, setLocale, t],
  );

  return (
    <I18nContext.Provider value={value}>
      {children}
    </I18nContext.Provider>
  );
};

export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext);
  if (!context) {
    throw new Error('useI18n must be used within an I18nProvider');
  }
  return context;
}

interface LanguageSwitcherProps {
  id: string;
  className?: string;
}

export const LanguageSwitcher: React.FC<LanguageSwitcherProps> = ({
  id,
  className = '',
}) => {
  const { locale, setLocale, t } = useI18n();

  const handleChange = (event: React.ChangeEvent<HTMLSelectElement>) => {
    const nextLocale = event.target.value;
    if (isSupportedLocale(nextLocale)) {
      setLocale(nextLocale);
    }
  };

  return (
    <label className={`inline-flex items-center gap-1.5 ${className}`} htmlFor={id}>
      <span aria-hidden="true">🌐</span>
      <span className="sr-only">{t('language.label')}</span>
      <select
        id={id}
        aria-label={t('language.label')}
        value={locale}
        onChange={handleChange}
        className="rounded-md border border-gray-300 bg-white px-2 py-1.5 text-sm text-gray-700 focus:outline-none focus:ring-2 focus:ring-blue-500"
      >
        <option value="en-US">English</option>
        <option value="zh-CN">简体中文</option>
      </select>
    </label>
  );
};
