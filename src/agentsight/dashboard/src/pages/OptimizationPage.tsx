import React, { Fragment, useCallback, useEffect, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import {
  fetchSessions,
  fetchOptimizeResults,
  runOptimizeDimension,
  ApiRequestError,
} from '../utils/apiClient';
import type { SessionSummary } from '../utils/apiClient';
import type {
  AccIssue,
  AccuracyResult,
  AnalysisReport,
  CostStats,
  Failure,
  PerfReport,
  PerfStats,
  WasteReport,
} from '../types/optimization';
import TokenFlameChart from '../components/TokenFlameChart';
import { OptimizationSettings } from '../components/OptimizationSettings';

// ── 通用状态类型 ──────────────────────────────────────────────────────────────

type DimState = 'idle' | 'loading' | 'done' | 'error';
type AnalysisProgress = {
  perf: DimState;
  perfIssues: DimState;
  cost: DimState;
  costWaste: DimState;
  accuracy: DimState;
};
type AnalysisTab = 'accuracy' | 'perf' | 'cost';

const FAILURE_LABELS: Record<string, string> = {
  tool_error: '工具错误',
  reasoning_error: '推理错误',
  timeout: '超时',
  invalid_usage: '无效调用',
};

// ── 工具函数 ──────────────────────────────────────────────────────────────────

const H = (s: string) => (
  <span
    className="[&_code]:bg-gray-100 [&_code]:px-1 [&_code]:py-0.5 [&_code]:rounded [&_code]:font-mono [&_code]:text-xs [&_code]:text-gray-800"
    dangerouslySetInnerHTML={{ __html: s }}
  />
);

function formatSecs(s: number): string {
  if (s >= 60) {
    const m = Math.floor(s / 60);
    const sec = Math.round(s % 60);
    return `${m}m ${sec}s`;
  }
  return `${s.toFixed(1)}s`;
}

function fmtTokens(n: number): string {
  return n.toLocaleString();
}

function shortId(id: string, len = 20): string {
  return id.length > len ? id.slice(0, len) + '…' : id;
}

/** ns 时间戳 → 本地时间字符串 */
function fmtNs(ns: number): string {
  return new Date(ns / 1e6).toLocaleString();
}

const Spinner: React.FC<{ size?: number }> = ({ size = 18 }) => (
  <span
    className="inline-block rounded-full border-2 border-blue-500 border-t-transparent animate-spin flex-shrink-0"
    style={{ width: size, height: size }}
  />
);

// ── Section 头部 ──────────────────────────────────────────────────────────────

const SEC_TAG_CLS: Record<string, string> = {
  acc: 'bg-green-100 text-green-700',
  perf: 'bg-blue-100 text-blue-700',
  cost: 'bg-amber-100 text-amber-700',
};

const SectionHead: React.FC<{ idx: string; title: string; tag?: string; tagKind?: string }> = ({
  idx,
  title,
  tag,
  tagKind = 'acc',
}) => (
  <div className="flex items-center gap-2.5 mb-3">
    <span className="w-7 h-7 rounded-lg bg-gray-900 text-white flex items-center justify-center font-mono font-bold text-sm flex-shrink-0">
      {idx}
    </span>
    <h2 className="text-base font-semibold text-gray-900">{title}</h2>
    {tag && (
      <span className={`px-2 py-0.5 rounded text-xs font-medium ${SEC_TAG_CLS[tagKind] ?? SEC_TAG_CLS.acc}`}>
        {tag}
      </span>
    )}
  </div>
);

// ── 加载中 / 失败 / 未分析 占位 ───────────────────────────────────────────────

function LoadingBlock({ label }: { label: string }) {
  return (
    <section className="mt-6">
      <SectionHead idx="…" title={label} tag="分析中" />
      <div className="bg-white rounded-lg shadow border border-gray-200 flex items-center gap-3 px-6 py-5">
        <Spinner size={20} />
        <span className="text-gray-500 font-mono text-[13px]">正在计算，请稍候...</span>
      </div>
    </section>
  );
}

function ErrorBlock({ label }: { label: string }) {
  return (
    <section className="mt-6">
      <SectionHead idx="✗" title={label} />
      <div className="bg-red-50 border border-red-200 text-red-700 px-6 py-4 rounded-lg text-sm">
        分析失败 —— 可稍后点击「重新分析」重试。
      </div>
    </section>
  );
}

function IdleBlock({ label }: { label: string }) {
  return (
    <section className="mt-6">
      <SectionHead idx="·" title={label} />
      <div className="bg-white rounded-lg shadow border border-gray-200 px-6 py-8 text-center text-gray-400 text-sm">
        该维度尚未分析 —— 点击右上角「开始分析」运行。
      </div>
    </section>
  );
}

// ── 准确性维度：五字段归因表（移植自 agentopt Dimensions.tsx）────────────────

// 根因主因「在原地能修」时对应的修复落点；fixLocus 与之不同 = 分叉（锅在 A、改在 B）
const SAME_PLACE: Record<string, string> = {
  Skill: 'Skill',
  Context: 'Context-policy',
  Tool: 'Tool',
  Model: 'Model-routing',
  Env: '无',
  Input: '无',
  Orchestration: '', // 编排问题无「原地」修复点，落到 Skill 即视为分叉
};

const CONF_CLS: Record<AccIssue['confidence'], string> = {
  高: 'bg-green-100 text-green-700',
  中: 'bg-yellow-100 text-yellow-700',
  低: 'bg-gray-100 text-gray-500',
};

// 构造可直接喂给 Agent 自我优化的优化提示词
function buildOptPrompt(it: AccIssue): string {
  const rc = it.rootCause.map((r) => r.object).join('、');
  return `你是一名 Agent Skill 优化专家。请根据以下诊断结论，修改对应的 Skill 定义以解决问题。

## 问题诊断

- **现象**：${it.symptom}
- **缺陷类型**：${it.defectType}
- **归因对象**：${rc}
- **修复落点**：${it.fixLocus}
- **发生位置**：${it.at}

## 证据

${it.detail.replace(/<\/?code>/g, '\`')}

## 优化建议

${it.fix.replace(/<\/?code>/g, '\`')}

请直接输出修改后的 Skill 内容（或差异补丁），并说明修改理由。`;
}

// 复制按钮：复制后显示 ✓ 2秒
function CopyBtn({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  const handleCopy = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      const ok = () => {
        setDone(true);
        setTimeout(() => setDone(false), 2000);
      };
      if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).then(ok).catch(() => fallbackCopy(text, ok));
      } else {
        fallbackCopy(text, ok);
      }
    },
    [text]
  );
  return (
    <button
      onClick={handleCopy}
      className={`px-2 py-1 rounded text-xs whitespace-nowrap transition-colors ${
        done
          ? 'bg-green-100 text-green-600'
          : 'bg-gray-100 hover:bg-gray-200 text-gray-500 hover:text-gray-700'
      }`}
      title={done ? '已复制' : '复制优化提示词'}
    >
      {done ? '✓ 已复制' : '⧉ 复制'}
    </button>
  );
}

function fallbackCopy(text: string, done: () => void) {
  const el = document.createElement('textarea');
  el.value = text;
  el.style.position = 'fixed';
  el.style.opacity = '0';
  document.body.appendChild(el);
  el.focus();
  el.select();
  try { document.execCommand('copy'); } catch { /* ignore */ }
  document.body.removeChild(el);
  done();
}

function IssueTable({ issues }: { issues: AccIssue[] }) {
  const [open, setOpen] = useState<number | null>(0);
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[720px] text-sm">
        <thead>
          <tr className="border-b border-gray-200">
            <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600">现象</th>
            <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600 whitespace-nowrap">缺陷类型</th>
            <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600 whitespace-nowrap">归因对象</th>
            <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600 whitespace-nowrap">修复落点</th>
            <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600 whitespace-nowrap">置信度</th>
            <th className="text-left pb-2 text-xs font-semibold text-gray-600 whitespace-nowrap">优化提示词</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-100">
          {issues.map((it, i) => {
            const isOpen = open === i;
            const primary = it.rootCause.find((rc) => rc.role === '主因')?.object ?? '';
            const fixDiverges = it.fixLocus !== SAME_PLACE[primary];
            return (
              <Fragment key={i}>
                <tr
                  className={`cursor-pointer transition-colors ${isOpen ? 'bg-blue-50' : 'hover:bg-gray-50'} ${
                    it.optimizable ? '' : 'opacity-60'
                  }`}
                  onClick={() => setOpen(isOpen ? null : i)}
                >
                  <td className="py-2.5 pr-3 text-gray-800">
                    <span className="text-gray-400 text-xs mr-1.5">{isOpen ? '▼' : '▶'}</span>
                    {it.symptom}
                    {it.recovered && (
                      <span className="ml-1.5 px-1.5 py-0.5 rounded bg-amber-100 text-amber-700 text-[10px] whitespace-nowrap">
                        已恢复 · 优化线索
                      </span>
                    )}
                  </td>
                  <td className="py-2.5 pr-3 whitespace-nowrap">
                    <span className="px-1.5 py-0.5 rounded bg-gray-100 text-gray-700 font-mono text-xs">
                      {it.defectType}
                    </span>
                  </td>
                  <td className="py-2.5 pr-3 whitespace-nowrap">
                    {it.rootCause.map((rc) => (
                      <span
                        className="px-1.5 py-0.5 rounded bg-blue-100 text-blue-700 text-xs mr-1"
                        key={rc.object}
                      >
                        {rc.object}
                      </span>
                    ))}
                  </td>
                  <td className="py-2.5 pr-3 whitespace-nowrap">
                    <span
                      className={`text-xs font-medium ${fixDiverges ? 'text-amber-600' : 'text-gray-600'}`}
                      title={fixDiverges ? `锅在 ${primary}，改在 ${it.fixLocus}` : '与根因主因一致'}
                    >
                      → {it.fixLocus}
                    </span>
                  </td>
                  <td className="py-2.5 pr-3 whitespace-nowrap">
                    <span className={`px-1.5 py-0.5 rounded text-xs font-medium ${CONF_CLS[it.confidence]}`}>
                      {it.confidence}
                    </span>
                  </td>
                  <td className="py-2.5 whitespace-nowrap">
                    {it.optimizable ? (
                      <CopyBtn text={buildOptPrompt(it)} />
                    ) : (
                      <span className="px-2 py-1 rounded bg-gray-100 text-gray-400 text-xs">不可优化</span>
                    )}
                  </td>
                </tr>
                {isOpen && (
                  <tr className="bg-gray-50">
                    <td colSpan={6} className="px-4 py-3">
                      <dl className="text-sm space-y-2">
                        <div>
                          <dt className="text-xs font-semibold text-gray-500 uppercase tracking-wide">证据</dt>
                          <dd className="mt-1 text-gray-700">
                            {H(it.detail)}{' '}
                            <span className="font-mono text-xs text-gray-400">{it.at}</span>
                          </dd>
                        </div>
                        <div>
                          <dt className="text-xs font-semibold text-gray-500 uppercase tracking-wide">验证</dt>
                          <dd className="mt-1 text-gray-700">{it.verify}</dd>
                        </div>
                        <div>
                          <dt className="text-xs font-semibold text-gray-500 uppercase tracking-wide">修复</dt>
                          <dd className="mt-1 text-gray-700">{H(it.fix)}</dd>
                        </div>
                      </dl>
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

// ── 准确性维度 Section ────────────────────────────────────────────────────────

function FailureRow({ f, index }: { f: Failure; index: number }) {
  return (
    <div className="flex items-start gap-3 mb-3">
      <span
        className={`px-2 py-0.5 rounded text-xs font-medium flex-shrink-0 ${
          f.recovery ? 'bg-yellow-100 text-yellow-700' : 'bg-red-100 text-red-700'
        }`}
      >
        {FAILURE_LABELS[f.failure_type] ?? f.failure_type}
      </span>
      <div className="text-sm">
        <p className="font-semibold text-gray-800">#{index + 1} {f.description}</p>
        <p className="mt-1.5 text-gray-600">{f.context}</p>
        {f.recovery && <p className="mt-1.5 text-green-600">恢复: {f.recovery}</p>}
      </div>
    </div>
  );
}

function AccuracySection({ issues, failures }: { issues: AccIssue[]; failures: Failure[] }) {
  const count = issues.length > 0 ? issues.length : failures.length;
  return (
    <section className="mt-6">
      <SectionHead idx="A" title="准确性剖析" tag={`${count} 个问题`} tagKind="acc" />
      {count === 0 ? (
        <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
          <p className="text-green-600 font-mono text-sm">✓ 未检测到影响最终产出准确性的问题。</p>
        </div>
      ) : issues.length > 0 ? (
        // 五字段正交归因表（现象 / 缺陷类型 / 归因对象 / 修复落点 / 置信度 / 可优化）
        <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
          <h3 className="text-sm font-semibold text-gray-700 mb-3">失败清单 · 点击任意行展开根因与修复</h3>
          <IssueTable issues={issues} />
          <p className="text-[11px] text-gray-400 mt-3">
            症状 ≠ 病灶 ≠ 处方：缺陷类型是症状，归因对象是病灶，修复落点是处方。置信度由证据层级派生，可优化为规则派生闸门。
          </p>
        </div>
      ) : (
        // 兼容旧会话：无五字段归因时回退到旧失败清单
        <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
          <h3 className="text-sm font-semibold text-gray-700 mb-3">失败清单</h3>
          {failures.map((f, i) => (
            <FailureRow key={i} f={f} index={i} />
          ))}
        </div>
      )}
    </section>
  );
}

// ── 性能维度 ──────────────────────────────────────────────────────────────────

// 大类配色（agentsight 主题：推理=蓝 / 工具=绿 / 用户空闲=橙）
const PERF_CAT_COLOR: Record<string, string> = {
  模型推理慢: '#3b82f6',
  工具执行慢: '#10b981',
  用户空闲: '#f59e0b',
};

function PerfIssueTable({ issues, state }: { issues: PerfReport | null; state: DimState }) {
  // 加载中 / 失败 / 未分析态
  if (state !== 'done' || !issues) {
    return (
      <div className="bg-white rounded-lg shadow border border-gray-200 p-5 mt-4">
        <h3 className="text-sm font-semibold text-gray-700">优化策略 · LLM 选择</h3>
        {state === 'error' ? (
          <p className="text-red-500 font-mono text-[13px] mt-2">识别失败 —— LLM 调用出错，可稍后重试。</p>
        ) : state === 'loading' ? (
          <div className="flex items-center gap-3 py-2 mt-1">
            <Spinner />
            <span className="text-gray-500 font-mono text-[13px]">LLM 正在分析性能数据并选择优化策略…</span>
          </div>
        ) : (
          <p className="text-gray-400 font-mono text-[13px] mt-2">尚未分析 —— 点击「重新分析」运行 LLM 策略选择。</p>
        )}
      </div>
    );
  }

  if (issues.items.length === 0) {
    return (
      <div className="bg-white rounded-lg shadow border border-gray-200 p-5 mt-4">
        <h3 className="text-sm font-semibold text-gray-700">优化策略 · LLM 选择</h3>
        <p className="text-green-600 font-mono text-[13px] mt-2">
          ✓ LLM 分析了 {issues.considered} 项数据信号，未发现适用的优化策略。
        </p>
      </div>
    );
  }

  return (
    <div className="bg-white rounded-lg shadow border border-gray-200 p-5 mt-4">
      <div className="flex items-baseline justify-between flex-wrap gap-2">
        <h3 className="text-sm font-semibold text-gray-700 m-0">优化策略 · LLM 选择</h3>
        <span className="text-[11px] text-gray-400 font-mono">
          {issues.items.length} 条策略 · 共分析 {issues.considered} 项信号
        </span>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full min-w-[700px] mt-3.5 text-sm">
          <thead>
            <tr className="border-b border-gray-200">
              <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600">现象</th>
              <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600">策略类型</th>
              <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600">根因</th>
              <th className="text-left pb-2 pr-3 text-xs font-semibold text-gray-600">优化策略</th>
              <th className="text-right pb-2 text-xs font-semibold text-gray-600">预估节省</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {issues.items.map((it, i) => (
              <tr key={i}>
                <td className="py-2.5 pr-3 text-gray-800">
                  {it.symptom}
                  {it.at && <span className="text-[10px] text-gray-400 font-mono"> · {it.at}</span>}
                </td>
                <td className="py-2.5 pr-3">
                  <span className="inline-flex items-center gap-1.5 whitespace-nowrap">
                    <i
                      className="w-[9px] h-[9px] rounded-sm flex-shrink-0"
                      style={{ background: PERF_CAT_COLOR[it.category] ?? '#3b82f6' }}
                    />
                    <span className="font-mono text-xs text-gray-700">{it.subtype}</span>
                    <span className="text-[10px] text-gray-400">· {it.confidence}</span>
                  </span>
                </td>
                <td className="py-2.5 pr-3 text-xs text-gray-500">{it.root_cause || it.evidence}</td>
                <td className="py-2.5 pr-3 text-xs text-gray-700">{it.optimization}</td>
                <td className="py-2.5 text-right font-mono text-xs text-blue-600 whitespace-nowrap">
                  {formatSecs(it.impact_secs)}
                  <span className="text-gray-400"> · {Math.round(it.pct)}%</span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="text-[11px] text-gray-400 mt-3 font-mono">
        「优化策略」及根因/建议由 LLM 从策略目录中选择并具体化；时间数据由 Rust 从轨迹时间戳按「工具执行 / 模型推理 / 用户空闲」正交三分算出。
      </p>
    </div>
  );
}

function PerfSection({ perf, issues, issuesState }: { perf: PerfStats; issues: PerfReport | null; issuesState: DimState }) {
  const wall = perf.wall_secs || 1;
  const modelPct = Math.round((perf.model_secs / wall) * 100);
  const toolPct = Math.round((perf.tool_secs / wall) * 100);
  const idlePct = Math.max(0, 100 - modelPct - toolPct);

  return (
    <section className="mt-6">
      <SectionHead idx="P" title="性能剖析" tag={formatSecs(perf.wall_secs)} tagKind="perf" />

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
          <h3 className="text-sm font-semibold text-gray-700 mb-3">时间分布</h3>
          <div className="flex h-8 rounded-lg overflow-hidden">
            <div
              className="flex items-center justify-center text-white text-xs whitespace-nowrap overflow-hidden"
              style={{ flex: modelPct || 0.0001, background: '#3b82f6' }}
            >
              {modelPct > 15 ? `模型 ${modelPct}%` : ''}
            </div>
            <div
              className="flex items-center justify-center text-white text-xs whitespace-nowrap overflow-hidden"
              style={{ flex: toolPct || 0.0001, background: '#10b981' }}
            >
              {toolPct > 10 ? `工具 ${toolPct}%` : ''}
            </div>
            {idlePct > 0 && (
              <div
                className="flex items-center justify-center text-white text-xs whitespace-nowrap overflow-hidden"
                style={{ flex: idlePct, background: '#9ca3af' }}
              >
                {idlePct > 15 ? `空闲 ${idlePct}%` : ''}
              </div>
            )}
          </div>
          <div className="flex flex-wrap gap-4 mt-2.5 text-xs text-gray-500">
            <span className="flex items-center gap-1.5">
              <i className="w-2.5 h-2.5 rounded-sm bg-blue-500" /> 模型推理 {formatSecs(perf.model_secs)}
            </span>
            <span className="flex items-center gap-1.5">
              <i className="w-2.5 h-2.5 rounded-sm bg-emerald-500" /> 工具执行 {formatSecs(perf.tool_secs)}
            </span>
            {perf.idle_secs > 1 && (
              <span className="flex items-center gap-1.5">
                <i className="w-2.5 h-2.5 rounded-sm bg-gray-400" /> 用户空闲 {formatSecs(perf.idle_secs)}
              </span>
            )}
          </div>
          <p className="text-sm text-gray-600 mt-3">
            共 <b>{perf.tool_count}</b> 次工具调用，总耗时 <b>{formatSecs(perf.wall_secs)}</b>
          </p>
        </div>

        <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
          <h3 className="text-sm font-semibold text-gray-700 mb-3">最慢调用</h3>
          <table className="w-full table-fixed text-sm">
            <thead>
              <tr className="border-b border-gray-200">
                <th className="w-[90px] text-left pb-2 pr-3 text-xs font-semibold text-gray-600">工具</th>
                <th className="w-[70px] text-left pb-2 pr-3 text-xs font-semibold text-gray-600">耗时</th>
                <th className="text-left pb-2 text-xs font-semibold text-gray-600">命令</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-100">
              {perf.top_slow.slice(0, 5).map((call, i) => (
                <tr key={i}>
                  <td className="py-2 pr-3 whitespace-nowrap text-gray-800">{call.name}</td>
                  <td className={`py-2 pr-3 whitespace-nowrap ${call.err ? 'text-red-500' : 'text-gray-600'}`}>
                    {formatSecs(call.dur)}{call.err ? ' ✗' : ''}
                  </td>
                  <td
                    title={call.cmd}
                    className="py-2 text-[11px] font-mono text-gray-500 overflow-hidden text-ellipsis whitespace-nowrap"
                  >
                    {call.cmd}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* 优化策略表（Rust 供数 → LLM 选择适用策略 + 根因/建议）*/}
      <PerfIssueTable issues={issues} state={issuesState} />
    </section>
  );
}

// ── 成本维度 ──────────────────────────────────────────────────────────────────

function CostSection({ cost, waste, wasteState }: { cost: CostStats; waste: WasteReport | null; wasteState: DimState }) {
  const h = cost.headroom;
  const fmtK = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : `${Math.round(n)}`);

  const hrSavePct = h?.headroom_save_pct ?? 0;
  const isReal = hrSavePct > 0;
  const usageSteps = cost.usage_steps ?? 0;
  const totalSteps = cost.calls?.length ?? 0;
  const tokSource = usageSteps > 0
    ? usageSteps === totalSteps ? 'token 实测(usage)' : `token 实测 ${usageSteps}/${totalSteps} 步`
    : 'token 估算';

  return (
    <section className="mt-6">
      <SectionHead
        idx="$"
        title="成本剖析"
        tagKind="cost"
        tag={
          h
            ? isReal
              ? `${fmtK(h.total_input_tok + h.total_output_tok)} tok · Headroom 实测 -${hrSavePct.toFixed(0)}%`
              : `${fmtK(h.total_input_tok + h.total_output_tok)} tok · 可优化 ${h.pct.toFixed(0)}%`
            : `${cost.total_events} 事件 · ${cost.total_chars.toLocaleString()} 字符`
        }
      />

      {/* Headroom summary card */}
      {h && (h.total_input_tok + h.total_output_tok) > 0 && (
        <div className="grid grid-cols-2 gap-3 mb-4">
          <div className="bg-white rounded-lg shadow border border-gray-200 px-4 py-3.5">
            <div className="font-mono text-[10px] tracking-widest uppercase text-gray-500">内容体积</div>
            <div className="text-[22px] font-bold mt-1 text-amber-600 leading-tight">
              {cost.total_chars.toLocaleString()} <span className="text-[13px] font-normal">字符</span>
            </div>
            <div className="text-[11px] text-gray-500 mt-1 font-mono">
              {cost.total_events} 事件 · {cost.calls?.length ?? 0} 步 LLM 调用 · {tokSource}
            </div>
          </div>
          <div className="bg-white rounded-lg shadow border border-gray-200 px-4 py-3.5">
            <div className={`font-mono text-[10px] tracking-widest uppercase ${isReal ? 'text-emerald-600' : 'text-gray-500'}`}>
              {isReal ? 'Headroom 实测' : 'Optimization Headroom'}
            </div>
            <div className="text-[22px] font-bold mt-1 text-emerald-600 leading-tight">
              {isReal ? `-${hrSavePct.toFixed(0)}%` : `${h.pct.toFixed(0)}%`}
            </div>
            <div className="text-[11px] text-gray-500 mt-1 font-mono">
              {isReal
                ? `压缩后 ${fmtK(h.headroom_compressed_tok ?? 0)} tok`
                : `payload 可优化 ${fmtK(h.payload_deletable_tok + h.payload_cacheable_tok)} tok`}
            </div>
          </div>
        </div>
      )}

      {/* Token 火焰图：逐步 context window 重放拆解 + LLM 浪费诊断 */}
      <TokenFlameChart cost={cost} waste={waste} wasteState={wasteState} />
    </section>
  );
}

// ── 三 TAB 渐进式分析视图（移植自 agentopt AnalysisView.tsx）─────────────────

const TAB_DEFS: { key: AnalysisTab; name: string; label: string }[] = [
  { key: 'accuracy', name: '准确性', label: '准确性剖析' },
  { key: 'perf', name: '性能', label: '性能剖析' },
  { key: 'cost', name: '成本', label: '成本剖析' },
];

const TAB_DOT: Record<AnalysisTab, string> = {
  accuracy: 'bg-green-500',
  perf: 'bg-blue-500',
  cost: 'bg-amber-500',
};

function AnalysisView({ report, progress }: { report: AnalysisReport; progress: AnalysisProgress }) {
  const [tab, setTab] = useState<AnalysisTab>('perf');

  // Composite state per tab: perf/cost each have 2 phases (stats + LLM)
  function tabState(t: AnalysisTab): {
    indicator: 'idle' | 'loading' | 'partial' | 'done' | 'error';
    doneCount: number;
    totalCount: number;
  } {
    if (t === 'accuracy') {
      const s = progress.accuracy;
      return { indicator: s, doneCount: s === 'done' ? 1 : 0, totalCount: 1 };
    }
    const s1 = t === 'perf' ? progress.perf : progress.cost;
    const s2 = t === 'perf' ? progress.perfIssues : progress.costWaste;
    if (s1 === 'idle') return { indicator: 'idle', doneCount: 0, totalCount: 2 };
    if (s1 === 'error') return { indicator: 'error', doneCount: 0, totalCount: 2 };
    if (s1 === 'loading') return { indicator: 'loading', doneCount: 0, totalCount: 2 };
    // s1 done
    if (s2 === 'done') return { indicator: 'done', doneCount: 2, totalCount: 2 };
    if (s2 === 'error' || s2 === 'idle') return { indicator: 'done', doneCount: 2, totalCount: 2 }; // partial error/idle still shows content
    return { indicator: 'partial', doneCount: 1, totalCount: 2 };
  }

  const state = tabState(tab);
  const currentDef = TAB_DEFS.find((t) => t.key === tab)!;
  // Content is viewable once the base stats are done (even if LLM is still running)
  const contentReady = tab === 'accuracy'
    ? progress.accuracy === 'done'
    : (tab === 'perf' ? progress.perf === 'done' && !!report.perf : progress.cost === 'done' && !!report.cost);

  return (
    <>
      {/* 维度切换 Tab */}
      <div className="flex gap-1 bg-white rounded-lg shadow border border-gray-200 p-1 w-fit" role="tablist" aria-label="维度剖析">
        {TAB_DEFS.map((t) => {
          const ts = tabState(t.key);
          const active = tab === t.key;
          return (
            <button
              key={t.key}
              role="tab"
              aria-selected={active}
              className={`flex items-center px-4 py-2 rounded-md text-sm font-medium transition-colors ${
                active ? 'bg-blue-100 text-blue-700' : 'text-gray-600 hover:bg-gray-100'
              }`}
              onClick={() => setTab(t.key)}
            >
              <span className={`w-2 h-2 rounded-full mr-2 ${TAB_DOT[t.key]}`} />
              {t.name}
              {ts.indicator === 'loading' && (
                <span className="ml-1.5"><Spinner size={14} /></span>
              )}
              {ts.indicator === 'partial' && (
                <span className="ml-1.5 text-[11px] text-gray-500 font-mono inline-flex items-center gap-1">
                  {ts.doneCount}/{ts.totalCount}
                  <Spinner size={11} />
                </span>
              )}
              {ts.indicator === 'done' && <span className="ml-1.5 text-green-500 leading-none">✓</span>}
              {ts.indicator === 'error' && <span className="ml-1.5 text-red-500 leading-none">✗</span>}
            </button>
          );
        })}
      </div>

      {/* 当前 tab 内容 */}
      {!contentReady && state.indicator === 'error' && <ErrorBlock label={currentDef.label} />}
      {!contentReady && state.indicator === 'idle' && <IdleBlock label={currentDef.label} />}
      {!contentReady && (state.indicator === 'loading' || state.indicator === 'partial') && (
        <LoadingBlock label={currentDef.label} />
      )}
      {contentReady && tab === 'accuracy' && (
        <AccuracySection issues={report.issues ?? []} failures={report.failures} />
      )}
      {contentReady && tab === 'perf' && report.perf && (
        <PerfSection perf={report.perf} issues={report.perf_issues ?? null} issuesState={progress.perfIssues} />
      )}
      {contentReady && tab === 'cost' && report.cost && (
        <CostSection cost={report.cost} waste={report.cost_waste ?? null} wasteState={progress.costWaste} />
      )}
    </>
  );
}

// ── 会话分析视图 ──────────────────────────────────────────────────────────────

const EMPTY_REPORT: AnalysisReport = {
  extraction: { final_answer: '' },
  failures: [],
  issues: [],
  perf: null,
  perf_issues: null,
  cost: null,
  cost_waste: null,
};

const IDLE_PROGRESS: AnalysisProgress = {
  perf: 'idle',
  perfIssues: 'idle',
  cost: 'idle',
  costWaste: 'idle',
  accuracy: 'idle',
};

function SessionAnalysisView({ sessionId, onOpenSettings }: { sessionId: string; onOpenSettings: () => void }) {
  const navigate = useNavigate();
  const [report, setReport] = useState<AnalysisReport>(EMPTY_REPORT);
  const [progress, setProgress] = useState<AnalysisProgress>(IDLE_PROGRESS);
  const [loadingResults, setLoadingResults] = useState(true);
  const [llmNotConfigured, setLlmNotConfigured] = useState(false);
  const [analyzeError, setAnalyzeError] = useState<string | null>(null);

  // 进入分析页时先加载历史结果展示
  useEffect(() => {
    let cancelled = false;
    setLoadingResults(true);
    setReport(EMPTY_REPORT);
    setProgress(IDLE_PROGRESS);
    setAnalyzeError(null);
    setLlmNotConfigured(false);
    (async () => {
      try {
        const data = await fetchOptimizeResults(sessionId);
        if (cancelled) return;
        setReport({
          extraction: data.accuracy?.extraction ?? { final_answer: '' },
          failures: data.accuracy?.failures ?? [],
          issues: data.accuracy?.issues ?? [],
          perf: data.perf,
          perf_issues: data.perf_issues,
          cost: data.cost,
          cost_waste: data.cost_waste,
        });
        setProgress({
          perf: data.perf ? 'done' : 'idle',
          perfIssues: data.perf_issues ? 'done' : 'idle',
          cost: data.cost ? 'done' : 'idle',
          costWaste: data.cost_waste ? 'done' : 'idle',
          accuracy: data.accuracy ? 'done' : 'idle',
        });
      } catch {
        // 无历史结果时保持 idle，不阻塞页面
      } finally {
        if (!cancelled) setLoadingResults(false);
      }
    })();
    return () => { cancelled = true; };
  }, [sessionId]);

  // 维度请求失败的统一处理：400 llm_not_configured 时提示去设置里配置 LLM
  const handleDimError = useCallback((e: unknown) => {
    if (e instanceof ApiRequestError && e.status === 400 && e.body?.error === 'llm_not_configured') {
      setLlmNotConfigured(true);
    }
  }, []);

  // 渐进式分析：并行触发 5 个维度，每个维度独立更新 loading/done/error
  const runAnalysis = useCallback(() => {
    setReport(EMPTY_REPORT);
    setProgress({ perf: 'loading', perfIssues: 'loading', cost: 'loading', costWaste: 'loading', accuracy: 'loading' });
    setAnalyzeError(null);
    setLlmNotConfigured(false);

    // perf — 纯计算，毫秒级
    runOptimizeDimension<PerfStats>(sessionId, 'perf')
      .then((data) => {
        setReport((prev) => ({ ...prev, perf: data }));
        setProgress((prev) => ({ ...prev, perf: 'done' }));
      })
      .catch((e) => { handleDimError(e); setProgress((prev) => ({ ...prev, perf: 'error' })); });

    // perf-issues — Rust 供数 + LLM 策略选择，10-30s
    runOptimizeDimension<PerfReport>(sessionId, 'perf-issues')
      .then((data) => {
        setReport((prev) => ({ ...prev, perf_issues: data }));
        setProgress((prev) => ({ ...prev, perfIssues: 'done' }));
      })
      .catch((e) => { handleDimError(e); setProgress((prev) => ({ ...prev, perfIssues: 'error' })); });

    // cost — 纯计算，毫秒级
    runOptimizeDimension<CostStats>(sessionId, 'cost')
      .then((data) => {
        setReport((prev) => ({ ...prev, cost: data }));
        setProgress((prev) => ({ ...prev, cost: 'done' }));
      })
      .catch((e) => { handleDimError(e); setProgress((prev) => ({ ...prev, cost: 'error' })); });

    // cost-waste — Rust 候选 + LLM 判定，10-30s
    runOptimizeDimension<WasteReport>(sessionId, 'cost-waste')
      .then((data) => {
        setReport((prev) => ({ ...prev, cost_waste: data }));
        setProgress((prev) => ({ ...prev, costWaste: 'done' }));
      })
      .catch((e) => { handleDimError(e); setProgress((prev) => ({ ...prev, costWaste: 'error' })); });

    // accuracy — LLM 多检测器，30-60s+，不设短超时
    runOptimizeDimension<AccuracyResult>(sessionId, 'accuracy')
      .then((data) => {
        setReport((prev) => ({
          ...prev,
          extraction: data.extraction,
          failures: data.failures,
          issues: data.issues ?? [],
        }));
        setProgress((prev) => ({ ...prev, accuracy: 'done' }));
      })
      .catch((e) => {
        handleDimError(e);
        setProgress((prev) => ({ ...prev, accuracy: 'error' }));
        setAnalyzeError(`准确性分析失败: ${e instanceof Error ? e.message : String(e)}`);
      });
  }, [sessionId, handleDimError]);

  const running = Object.values(progress).some((s) => s === 'loading');
  const hasAnyResult = Object.values(progress).some((s) => s === 'done');

  return (
    <main className="max-w-screen-xl mx-auto px-6 py-6">
      {/* ── Header bar ── */}
      <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-4 flex flex-wrap items-center gap-3">
        <button
          onClick={() => navigate('/optimization')}
          className="px-3 py-1.5 text-sm bg-gray-100 hover:bg-gray-200 rounded-lg text-gray-600 transition-colors"
        >
          ← 返回会话列表
        </button>
        <div className="min-w-0">
          <p className="text-xs text-gray-400">优化分析 · 会话</p>
          <p className="font-mono text-sm text-gray-800 truncate" title={sessionId}>{sessionId}</p>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <button
            onClick={onOpenSettings}
            className="px-4 py-2 bg-gray-100 hover:bg-gray-200 text-gray-600 rounded-lg text-sm transition-colors"
          >
            ⚙️ LLM 设置
          </button>
          <button
            onClick={runAnalysis}
            disabled={running || loadingResults}
            className="px-5 py-2 bg-blue-600 text-white rounded-lg text-sm font-medium hover:bg-blue-700 transition-colors disabled:opacity-50"
          >
            {running ? '分析中...' : hasAnyResult ? '重新分析' : '开始分析'}
          </button>
        </div>
      </div>

      {/* ── LLM 未配置提示 ── */}
      {llmNotConfigured && (
        <div className="mt-4 bg-yellow-50 border border-yellow-200 text-yellow-800 px-4 py-3 rounded-lg text-sm flex items-center gap-2 flex-wrap">
          <span>LLM 尚未配置 —— 性能策略 / 成本浪费 / 准确性维度需要调用 LLM。</span>
          <button onClick={onOpenSettings} className="underline font-medium hover:text-yellow-900">
            前往 LLM 设置
          </button>
        </div>
      )}

      {/* ── 准确性错误 ── */}
      {analyzeError && !llmNotConfigured && (
        <div className="mt-4 bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded-lg text-sm break-all">
          {analyzeError}
        </div>
      )}

      {/* ── 摘要行 ── */}
      <div className="mt-4 mb-4 text-sm text-gray-500">
        {progress.accuracy === 'done'
          ? `${(report.issues?.length ?? 0) || report.failures.length} 个问题`
          : progress.accuracy === 'loading' ? '准确性分析中...' : ''}
        {report.perf ? ` · ${report.perf.tool_count} 次工具调用 · ${Math.round(report.perf.wall_secs)}s` : ''}
        {report.cost ? ` · ${report.cost.total_events} 事件` : ''}
      </div>

      {/* ── 提取结果 ── */}
      {progress.accuracy === 'done' && report.extraction.final_answer && (
        <div className="mb-4 bg-white rounded-lg shadow border border-green-200 p-4">
          <p className="font-mono text-xs text-gray-600 m-0">
            <b>提取结果：</b>
            {report.extraction.final_answer.length > 200
              ? [...report.extraction.final_answer].slice(0, 200).join('') + '...'
              : report.extraction.final_answer}
          </p>
        </div>
      )}

      {/* ── 内容 ── */}
      {loadingResults ? (
        <div className="flex items-center gap-3 py-16 justify-center text-gray-400">
          <Spinner size={20} />
          <span className="text-sm">加载历史分析结果...</span>
        </div>
      ) : !hasAnyResult && !running ? (
        <div className="flex flex-col items-center justify-center py-20 text-gray-400">
          <div className="text-5xl mb-4">🔬</div>
          <p className="text-base">该会话尚未进行优化分析</p>
          <p className="text-xs mt-2">点击「开始分析」并行运行准确性 / 性能 / 成本三维度剖析</p>
        </div>
      ) : (
        <AnalysisView report={report} progress={progress} />
      )}
    </main>
  );
}

// ── 会话选择列表 ──────────────────────────────────────────────────────────────

const RANGE_OPTIONS = [
  { label: '最近 24 小时', hours: 24 },
  { label: '最近 7 天', hours: 24 * 7 },
  { label: '最近 30 天', hours: 24 * 30 },
];

function SessionListView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const navigate = useNavigate();
  const [rangeHours, setRangeHours] = useState(24 * 7);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    const now = Date.now();
    const startNs = (now - rangeHours * 3600 * 1000) * 1_000_000;
    const endNs = now * 1_000_000;
    fetchSessions(startNs, endNs)
      .then((data) => {
        if (cancelled) return;
        // 最近活跃在前
        setSessions([...data].sort((a, b) => b.last_seen_ns - a.last_seen_ns));
      })
      .catch((e) => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [rangeHours]);

  return (
    <main className="max-w-screen-xl mx-auto px-6 py-6 space-y-4">
      {/* ── Toolbar ── */}
      <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-4 flex flex-wrap items-center gap-3">
        <div>
          <h1 className="text-lg font-semibold text-gray-900">优化分析</h1>
          <p className="text-xs text-gray-400 mt-0.5">选择一个会话，运行准确性 / 性能 / 成本三维度剖析</p>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <select
            className="border border-gray-300 rounded-lg px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-blue-400"
            value={rangeHours}
            onChange={(e) => setRangeHours(Number(e.target.value))}
          >
            {RANGE_OPTIONS.map((o) => (
              <option key={o.hours} value={o.hours}>{o.label}</option>
            ))}
          </select>
          <button
            onClick={onOpenSettings}
            className="px-4 py-2 bg-gray-100 hover:bg-gray-200 text-gray-600 rounded-lg text-sm transition-colors"
          >
            ⚙️ LLM 设置
          </button>
        </div>
      </div>

      {error && (
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded-lg text-sm">
          加载会话失败: {error}
        </div>
      )}

      {/* ── Session table ── */}
      <div className="bg-white rounded-xl shadow-sm border border-gray-200 overflow-hidden">
        {loading ? (
          <div className="flex items-center gap-3 py-16 justify-center text-gray-400">
            <Spinner size={20} />
            <span className="text-sm">加载会话列表...</span>
          </div>
        ) : sessions.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 text-gray-400">
            <div className="text-4xl mb-3">📭</div>
            <p className="text-sm">所选时间范围内没有会话</p>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[800px]">
              <thead className="bg-gray-50 border-b border-gray-200">
                <tr>
                  <th className="px-4 lg:px-6 py-3 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide">会话 ID</th>
                  <th className="px-4 lg:px-6 py-3 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide">Agent</th>
                  <th className="px-4 lg:px-6 py-3 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide">模型</th>
                  <th className="px-4 lg:px-6 py-3 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide">对话数</th>
                  <th className="px-4 lg:px-6 py-3 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide">Tokens</th>
                  <th className="px-4 lg:px-6 py-3 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide">最后活跃</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {sessions.map((s) => (
                  <tr
                    key={s.session_id}
                    className="hover:bg-blue-50 transition-colors cursor-pointer"
                    onClick={() => navigate(`/optimization/${encodeURIComponent(s.session_id)}`)}
                  >
                    <td className="px-4 lg:px-6 py-3.5">
                      <span className="font-mono text-sm text-gray-800" title={s.session_id}>
                        {shortId(s.session_id)}
                      </span>
                    </td>
                    <td className="px-4 lg:px-6 py-3.5 text-sm text-gray-700">
                      <span className="truncate block max-w-[160px]" title={s.agent_name ?? ''}>
                        {s.agent_name ?? '-'}
                      </span>
                    </td>
                    <td className="px-4 lg:px-6 py-3.5 text-sm text-gray-500">
                      <span className="truncate block max-w-[180px] font-mono text-xs" title={s.model ?? ''}>
                        {s.model ?? '-'}
                      </span>
                    </td>
                    <td className="px-4 lg:px-6 py-3.5 text-sm text-gray-900 text-right">
                      {s.conversation_count}
                    </td>
                    <td className="px-4 lg:px-6 py-3.5 text-sm text-right">
                      <span className="text-gray-900">{fmtTokens(s.total_input_tokens + s.total_output_tokens)}</span>
                      <span className="text-xs text-gray-400 ml-1.5">
                        (in {fmtTokens(s.total_input_tokens)} / out {fmtTokens(s.total_output_tokens)})
                      </span>
                    </td>
                    <td className="px-4 lg:px-6 py-3.5 text-sm text-gray-500 whitespace-nowrap">
                      {fmtNs(s.last_seen_ns)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </main>
  );
}

// ── 主页面 ────────────────────────────────────────────────────────────────────

export const OptimizationPage: React.FC = () => {
  const { sessionId } = useParams<{ sessionId: string }>();
  const [showSettings, setShowSettings] = useState(false);

  return (
    <>
      {sessionId ? (
        <SessionAnalysisView sessionId={sessionId} onOpenSettings={() => setShowSettings(true)} />
      ) : (
        <SessionListView onOpenSettings={() => setShowSettings(true)} />
      )}
      {showSettings && <OptimizationSettings onClose={() => setShowSettings(false)} />}
    </>
  );
};
