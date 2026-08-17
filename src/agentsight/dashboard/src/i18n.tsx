import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
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
  'latency.title': 'Latency Metrics',
  'latency.agent': 'Agent',
  'latency.calls': 'Calls',
  'latency.streaming': 'Streaming',
  'latency.ttft': 'TTFT',
  'latency.tps': 'TPS',
  'latency.tpot': 'TPOT',
  'latency.e2e': 'E2E',
  'latency.p50': 'P50',
  'latency.p95': 'P95',
  'latency.p99': 'P99',
  'latency.loading': 'Loading latency metrics...',
  'latency.empty': 'No latency data in this range',
  'latency.error': 'Failed to load latency metrics',
  'latency.range24h': 'Last 24h',
  'latency.range7d': 'Last 7d',
  'latency.range30d': 'Last 30d',
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
    'latency.title': '延迟指标',
    'latency.agent': 'Agent',
    'latency.calls': '调用数',
    'latency.streaming': '流式调用',
    'latency.ttft': 'TTFT',
    'latency.tps': 'TPS',
    'latency.tpot': 'TPOT',
    'latency.e2e': 'E2E',
    'latency.p50': 'P50',
    'latency.p95': 'P95',
    'latency.p99': 'P99',
    'latency.loading': '正在加载延迟指标...',
    'latency.empty': '当前范围内暂无延迟数据',
    'latency.error': '延迟指标加载失败',
    'latency.range24h': '最近 24h',
    'latency.range7d': '最近 7d',
    'latency.range30d': '最近 30d',
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

const LOCALE_OPTIONS: Array<{ value: Locale; label: string }> = [
  { value: 'en-US', label: 'English' },
  { value: 'zh-CN', label: '简体中文' },
];

export const LanguageSwitcher: React.FC<LanguageSwitcherProps> = ({
  id,
  className = '',
}) => {
  const { locale, setLocale, t } = useI18n();
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;

    const closeOnPointerDown = (event: PointerEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
      }
    };

    document.addEventListener('pointerdown', closeOnPointerDown);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOnPointerDown);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [open]);

  const selectedLabel = LOCALE_OPTIONS.find((option) => option.value === locale)?.label ?? 'English';

  return (
    <div ref={containerRef} className={`relative inline-flex items-center gap-1.5 ${className}`}>
      <span aria-hidden="true">🌐</span>
      <button
        type="button"
        id={id}
        aria-label={`${t('language.label')}: ${selectedLabel}`}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={`${id}-menu`}
        onClick={() => setOpen((current) => !current)}
        className="inline-flex items-center gap-1 rounded-md border border-gray-300 bg-white px-2 py-1.5 text-sm text-gray-700 focus:outline-none focus:ring-2 focus:ring-blue-500"
      >
        {selectedLabel}
        <span aria-hidden="true" className="text-xs text-gray-400">▾</span>
      </button>
      {open && (
        <div
          id={`${id}-menu`}
          role="menu"
          aria-label={t('language.label')}
          className="absolute right-0 top-full z-50 mt-1 min-w-[120px] rounded-md border border-gray-200 bg-white p-1 shadow-lg"
        >
          {LOCALE_OPTIONS.map((option) => (
            <button
              key={option.value}
              type="button"
              role="menuitemradio"
              aria-checked={locale === option.value}
              onClick={() => {
                setLocale(option.value);
                setOpen(false);
              }}
              className={`block w-full rounded px-3 py-2 text-left text-sm ${
                locale === option.value
                  ? 'bg-blue-50 text-blue-700'
                  : 'text-gray-700 hover:bg-gray-100'
              }`}
            >
              {option.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
};
