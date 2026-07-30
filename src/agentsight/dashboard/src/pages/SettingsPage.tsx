import React from 'react';
import { LlmConfigForm } from '../components/OptimizationSettings';

/** Standalone settings page hosting global dashboard configuration sections. */
export const SettingsPage: React.FC = () => {
  return (
    <div className="max-w-3xl mx-auto px-6 py-8">
      <div className="mb-6">
        <h1 className="text-2xl font-bold text-gray-900">⚙️ 设置</h1>
        <p className="text-sm text-gray-500 mt-1">管理 Dashboard 的全局配置。</p>
      </div>

      <div className="space-y-6">
        <LlmConfigForm />
      </div>
    </div>
  );
};
