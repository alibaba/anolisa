import React, { useEffect, useState } from 'react';
import { fetchOptimizeConfig, saveOptimizeConfig } from '../utils/apiClient';
import type { OptimizeLlmConfig } from '../types/optimization';

// ── Provider presets ──────────────────────────────────────────────────────

interface Provider {
  id: string;
  name: string;
  icon: string;
  base_url: string;
  models: { id: string; name: string }[];
}

const PROVIDERS: Provider[] = [
  {
    id: 'dashscope',
    name: '阿里云 DashScope',
    icon: '☁️',
    base_url: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    models: [
      { id: 'qwen-plus', name: 'Qwen Plus' },
      { id: 'qwen-max', name: 'Qwen Max' },
      { id: 'qwen-turbo', name: 'Qwen Turbo' },
      { id: 'qwen-long', name: 'Qwen Long' },
      { id: 'glm-5.2', name: 'GLM 5.2 (推理)' },
      { id: 'deepseek-chat', name: 'DeepSeek Chat' },
      { id: 'deepseek-r1', name: 'DeepSeek R1 (推理)' },
    ],
  },
  {
    id: 'openai',
    name: 'OpenAI',
    icon: '🟢',
    base_url: 'https://api.openai.com/v1',
    models: [
      { id: 'gpt-4o', name: 'GPT-4o' },
      { id: 'gpt-4o-mini', name: 'GPT-4o Mini' },
      { id: 'o3', name: 'o3 (推理)' },
      { id: 'o3-mini', name: 'o3 Mini' },
      { id: 'gpt-4.1', name: 'GPT-4.1' },
    ],
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    icon: '🐋',
    base_url: 'https://api.deepseek.com/v1',
    models: [
      { id: 'deepseek-chat', name: 'DeepSeek Chat' },
      { id: 'deepseek-reasoner', name: 'DeepSeek Reasoner' },
    ],
  },
  {
    id: 'zhipu',
    name: '智谱 GLM',
    icon: '🔮',
    base_url: 'https://open.bigmodel.cn/api/paas/v4',
    models: [
      { id: 'glm-4-plus', name: 'GLM-4 Plus' },
      { id: 'glm-5.2', name: 'GLM 5.2 (推理)' },
      { id: 'glm-4-flash', name: 'GLM-4 Flash' },
    ],
  },
  {
    id: 'moonshot',
    name: 'Moonshot 月之暗面',
    icon: '🌙',
    base_url: 'https://api.moonshot.cn/v1',
    models: [
      { id: 'moonshot-v1-32k', name: 'Moonshot v1 32K' },
      { id: 'moonshot-v1-128k', name: 'Moonshot v1 128K' },
    ],
  },
  {
    id: 'custom',
    name: '自定义端点',
    icon: '⚙️',
    base_url: '',
    models: [],
  },
];

// ── Detect provider from base_url ─────────────────────────────────────────

function detectProvider(baseUrl: string): string {
  for (const p of PROVIDERS) {
    if (p.id === 'custom') continue;
    if (baseUrl.startsWith(p.base_url.replace(/\/v\d+$/, ''))) {
      return p.id;
    }
  }
  return 'custom';
}

// ── Shared input styles ───────────────────────────────────────────────────

const inputCls =
  'w-full border border-gray-300 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-400';

// ── Component ─────────────────────────────────────────────────────────────

/** LLM configuration modal for the optimization analysis feature. */
export const OptimizationSettings: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  const [config, setConfig] = useState<OptimizeLlmConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Form fields
  const [apiKey, setApiKey] = useState('');
  const [provider, setProvider] = useState('custom');
  const [baseUrl, setBaseUrl] = useState('');
  const [model, setModel] = useState('');
  const [customModel, setCustomModel] = useState('');
  const [isKnownModel, setIsKnownModel] = useState(true);
  const [showKey, setShowKey] = useState(false);

  const activeProvider = PROVIDERS.find(p => p.id === provider) ?? PROVIDERS[PROVIDERS.length - 1];
  const availableModels = activeProvider.models;

  useEffect(() => {
    (async () => {
      try {
        const data = await fetchOptimizeConfig();
        setConfig(data);
        setBaseUrl(data.base_url);
        setModel(data.model);

        // Auto-detect provider
        const pid = detectProvider(data.base_url);
        setProvider(pid);
        const p = PROVIDERS.find(x => x.id === pid);
        if (p && !p.models.some(m => m.id === data.model)) {
          setIsKnownModel(false);
          setCustomModel(data.model);
        }
        setError(null);
      } catch (e) {
        setError(`加载配置失败: ${e instanceof Error ? e.message : String(e)}`);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  // When provider changes, update base URL
  function handleProviderChange(id: string) {
    setProvider(id);
    const p = PROVIDERS.find(x => x.id === id);
    if (p && p.id !== 'custom') {
      setBaseUrl(p.base_url);
    }
  }

  // When provider changes, check if current model is in the list
  useEffect(() => {
    if (!model) return;
    const found = availableModels.some(m => m.id === model);
    if (found) {
      setIsKnownModel(true);
      setCustomModel('');
    } else if (provider !== 'custom') {
      // Model not in provider's list — switch to custom input
      setIsKnownModel(false);
      setCustomModel(model);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider, availableModels]);

  async function handleSave(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    setSaved(false);
    setError(null);

    const effectiveModel = isKnownModel ? model : customModel;

    try {
      const body: { api_key?: string; base_url?: string; model?: string } = {
        base_url: baseUrl,
        model: effectiveModel,
      };
      // Omit api_key when unchanged; keys containing masking dots are ignored by the backend.
      if (apiKey.trim()) {
        body.api_key = apiKey.trim();
      }
      const updated = await saveOptimizeConfig(body);
      setConfig(updated);
      setApiKey('');
      setSaved(true);
      setTimeout(() => setSaved(false), 3000);
    } catch (e2) {
      setError(`保存失败: ${e2 instanceof Error ? e2.message : String(e2)}`);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
    >
      <div
        className="bg-white rounded-xl shadow-xl w-full max-w-lg max-h-[90vh] overflow-y-auto"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-6 py-4 border-b border-gray-200 flex items-start justify-between">
          <div>
            <h2 className="text-lg font-semibold text-gray-900">LLM 配置</h2>
            <p className="text-xs text-gray-500 mt-0.5">
              选择云服务厂商和模型，修改后立即生效，无需重启。
            </p>
          </div>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 text-xl leading-none px-1"
            title="关闭"
          >
            ×
          </button>
        </div>

        {loading ? (
          <div className="flex items-center gap-3 px-6 py-10 text-gray-500 text-sm">
            <span className="inline-block w-5 h-5 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
            加载配置中...
          </div>
        ) : (
          <form onSubmit={handleSave} className="px-6 py-5 space-y-4">
            {/* Provider */}
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">云服务商</label>
              <select
                className={inputCls}
                value={provider}
                onChange={(e) => handleProviderChange(e.target.value)}
              >
                {PROVIDERS.map(p => (
                  <option key={p.id} value={p.id}>
                    {p.icon} {p.name}
                  </option>
                ))}
              </select>
            </div>

            {/* Base URL */}
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Base URL</label>
              {provider === 'custom' ? (
                <input
                  type="text"
                  className={inputCls}
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  placeholder="https://your-api-endpoint/v1"
                  spellCheck={false}
                />
              ) : (
                <div className="w-full bg-gray-50 border border-gray-200 rounded-lg px-3 py-2 text-sm text-gray-500 font-mono truncate">
                  {baseUrl}
                </div>
              )}
            </div>

            {/* API Key */}
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">API Key</label>
              <div className="flex items-center gap-2">
                <input
                  type={showKey ? 'text' : 'password'}
                  className={inputCls}
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder={config?.api_key ?? '输入 API Key'}
                  spellCheck={false}
                  autoComplete="off"
                />
                <button
                  type="button"
                  className="flex-shrink-0 px-2.5 py-2 bg-gray-100 hover:bg-gray-200 rounded-lg text-sm transition-colors"
                  onClick={() => setShowKey(!showKey)}
                  title={showKey ? '隐藏' : '显示'}
                >
                  {showKey ? '🙈' : '👁'}
                </button>
              </div>
              <p className="text-xs text-gray-400 mt-1">
                {config?.api_key ? '留空则保持当前配置不变' : '在对应厂商控制台获取 API Key'}
              </p>
            </div>

            {/* Model */}
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">模型</label>
              {provider !== 'custom' && availableModels.length > 0 ? (
                <>
                  <select
                    className={inputCls}
                    value={isKnownModel ? model : '__custom__'}
                    onChange={(e) => {
                      if (e.target.value === '__custom__') {
                        setIsKnownModel(false);
                      } else {
                        setIsKnownModel(true);
                        setModel(e.target.value);
                      }
                    }}
                  >
                    {availableModels.map(m => (
                      <option key={m.id} value={m.id}>{m.name} ({m.id})</option>
                    ))}
                    <option value="__custom__">✏️ 自定义模型名...</option>
                  </select>
                  {!isKnownModel && (
                    <input
                      type="text"
                      className={`${inputCls} mt-1.5`}
                      value={customModel}
                      onChange={(e) => setCustomModel(e.target.value)}
                      placeholder="输入模型 ID"
                      spellCheck={false}
                    />
                  )}
                </>
              ) : (
                <input
                  type="text"
                  className={inputCls}
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  placeholder="输入模型 ID，如 gpt-4o"
                  spellCheck={false}
                />
              )}
            </div>

            {error && (
              <div className="bg-red-50 border border-red-200 text-red-700 px-3 py-2 rounded-lg text-sm">
                {error}
              </div>
            )}

            {/* Actions */}
            <div className="flex items-center gap-3 pt-1">
              <button
                type="submit"
                disabled={saving}
                className="px-5 py-2 bg-blue-600 text-white rounded-lg text-sm font-medium hover:bg-blue-700 transition-colors disabled:opacity-50"
              >
                {saving ? '保存中...' : '保存配置'}
              </button>
              {saved && (
                <span className="text-sm text-green-600">✓ 配置已保存并生效</span>
              )}
              <button
                type="button"
                onClick={onClose}
                className="ml-auto px-4 py-2 bg-gray-100 hover:bg-gray-200 text-gray-600 rounded-lg text-sm transition-colors"
              >
                关闭
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
};
