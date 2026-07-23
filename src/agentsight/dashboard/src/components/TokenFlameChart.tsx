import React, { useMemo, useRef, useState, useEffect } from 'react';
import type { CostStats, LLMCall, WasteReport } from '../types/optimization';

type DimState = 'idle' | 'loading' | 'done' | 'error';

// ── 类别定义（与后端 LlmCall 字段一一对应）────────────────────────────────────
// 上柱：Context Window 组成（5 类，静态区域合并为一类）
type Cat = { key: string; label: string; color: string; get: (c: LLMCall) => number };

// 配色：agentsight 亮色主题（Tailwind 系）——冷（人类输入/静态）在下，暖（重放历史）在上。
const CONTEXT_CATS: Cat[] = [
  { key: 'static', label: '静态区域', color: '#6b7280', get: c => c.system_prompt + c.skill_definitions + c.tool_definitions }, // gray-500 · 恒定基岩
  { key: 'user', label: '用户提示词', color: '#3b82f6', get: c => c.user_messages },        // primary 蓝 · 人类输入
  { key: 'assistant', label: '助手输出', color: '#f59e0b', get: c => c.assistant_messages }, // amber-500 · O(n) 累积
  { key: 'tool', label: '工具输出', color: '#10b981', get: c => c.tool_results },            // emerald-500 · 沉积大头
  { key: 'others', label: '其它', color: '#8b5cf6', get: c => c.injected_context },          // violet-500 · 注入
];

// 下柱：载荷层可优化量（4 维镜像）
const OPT_CATS: Cat[] = [
  { key: 'cacheable', label: 'Cacheable 缓存', color: '#6366f1', get: c => c.cacheable },        // indigo-500
  { key: 'history', label: 'History 历史裁剪', color: '#14b8a6', get: c => c.history_prunable }, // teal-500
  { key: 'trim', label: 'Trimmable 工具截断', color: '#eab308', get: c => c.trimmable },         // yellow-500
  { key: 'prune', label: 'Prunable 注入裁剪', color: '#fb923c', get: c => c.prunable },          // orange-400
];

const OUTPUT_COLOR = '#ef4444'; // 输出曲线（danger 红）
const SAVE = '#10b981';         // 可优化 / 节省 强调色（success 绿）
const ROSE = '#ec4899';         // 可省此轮 描边（pink-500）
const AXIS = '#9ca3af';         // gray-400 轴文字
const GRID = '#e5e7eb';         // gray-200 网格线

// ── 工具函数 ──────────────────────────────────────────────────────────────────
const fmtK = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : `${Math.round(n)}`);
const ctxTotal = (c: LLMCall) => CONTEXT_CATS.reduce((s, cat) => s + cat.get(c), 0);
const optTotal = (c: LLMCall) => OPT_CATS.reduce((s, cat) => s + cat.get(c), 0);

// ── Summary Card ──────────────────────────────────────────────────────────────
function Card({ label, value, sub, accent }: { label: string; value: string; sub?: React.ReactNode; accent?: string }) {
  return (
    <div className="bg-white rounded-lg shadow border border-gray-200 px-4 py-3.5">
      <div className="font-mono text-[10px] tracking-widest uppercase text-gray-500">{label}</div>
      <div className="text-2xl font-bold mt-1.5 leading-tight" style={{ color: accent ?? '#111827' }}>{value}</div>
      {sub && <div className="text-[11px] text-gray-500 mt-1 font-mono">{sub}</div>}
    </div>
  );
}

const Spinner: React.FC<{ size?: number }> = ({ size = 18 }) => (
  <span
    className="inline-block rounded-full border-2 border-blue-500 border-t-transparent animate-spin flex-shrink-0"
    style={{ width: size, height: size }}
  />
);

// ── 主组件 ────────────────────────────────────────────────────────────────────
export default function TokenFlameChart({ cost, waste, wasteState }: { cost: CostStats; waste?: WasteReport | null; wasteState?: DimState }) {
  const calls = cost.calls ?? [];
  const [selected, setSelected] = useState<number>(() => Math.max(0, calls.length - 1));

  // 派生聚合（全部 hooks 必须在早退之前调用）
  const derived = useMemo(() => {
    // 空类别（整条轨迹求和为 0）不显示
    const visCtx = CONTEXT_CATS.filter(cat => calls.some(c => cat.get(c) > 0));
    const visOpt = OPT_CATS.filter(cat => calls.some(c => cat.get(c) > 0));

    const inputTokens = calls.reduce((s, c) => s + ctxTotal(c), 0); // 计费口径 input（含重放）
    const outputTokens = calls.reduce((s, c) => s + c.output_tokens, 0);

    let peakIdx = 0;
    let peakVal = 0;
    calls.forEach((c, i) => { const t = ctxTotal(c); if (t > peakVal) { peakVal = t; peakIdx = i; } });

    const maxCtx = Math.max(1, ...calls.map(ctxTotal));
    const maxOpt = Math.max(0, ...calls.map(optTotal));
    const maxOut = Math.max(1, ...calls.map(c => c.output_tokens));

    return { visCtx, visOpt, inputTokens, outputTokens, peakIdx, peakVal, maxCtx, maxOpt, maxOut };
  }, [calls]);

  // SVG 几何（自适应容器宽度）— hooks 需在早退前调用
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerW, setContainerW] = useState(600);
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(entries => {
      for (const e of entries) setContainerW(e.contentRect.width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 空状态
  if (calls.length === 0) {
    return (
      <div className="bg-white rounded-lg shadow border border-gray-200 p-5 mb-4">
        <h3 className="text-sm font-semibold text-gray-700">Token 火焰图</h3>
        <p className="text-gray-400 font-mono text-[13px] mt-2">
          该会话无逐步（per-step）成本数据 —— 可能是旧会话或轨迹解析失败。重新分析即可生成火焰图。
        </p>
      </div>
    );
  }

  const sel = calls[Math.min(selected, calls.length - 1)];

  // LLM 判定后的「值得优化」token 数（载荷层确定性 + 编排层需确认，分列）
  const wasteReady = wasteState === 'done' && !!waste;
  const llmSaveTok = wasteReady ? waste!.items.filter(i => !i.needs_confirm).reduce((s, i) => s + i.save_tokens, 0) : 0;
  const llmSuggestTok = wasteReady ? waste!.items.filter(i => i.needs_confirm).reduce((s, i) => s + i.save_tokens, 0) : 0;

  const { visCtx, visOpt, maxCtx, maxOpt, maxOut } = derived;
  const n = calls.length;
  const MIN_SLOT = 26;                    // 最小槽宽（多步时滚动）
  const SLOT = Math.max(MIN_SLOT, Math.floor((containerW - 64) / n)); // 自适应槽宽
  const BARW = Math.min(SLOT - 10, Math.max(16, SLOT * 0.6));         // 柱宽随槽宽缩放
  const ML = 48;                          // 左轴宽
  const H_TOP = 26;                       // 输出标注区
  const H_UP = 260;                       // 上柱区
  const scale = H_UP / maxCtx;            // px / token（上下共用，保证可比）
  const H_LOW = Math.min(140, Math.max(40, maxOpt * scale + 8));
  const H_X = 34;                         // x 轴标签
  const chartW = ML + n * SLOT + 16;
  const svgH = H_TOP + H_UP + H_LOW + H_X;
  const baseY = H_TOP + H_UP;             // 基线 y

  return (
    <div className="mb-5">
      {/* 1 · Summary Cards */}
      <div className="grid grid-cols-3 gap-3 mb-4">
        <Card
          label="Total Tokens"
          value={fmtK(derived.inputTokens + derived.outputTokens)}
          sub={<>in {fmtK(derived.inputTokens)} · out {fmtK(derived.outputTokens)}</>}
        />
        <Card
          label="LLM 判定可省"
          value={wasteReady ? fmtK(llmSaveTok) : '…'}
          sub={
            wasteReady
              ? (llmSuggestTok > 0 ? <>+ 编排层 {fmtK(llmSuggestTok)} tok · 需确认</> : `${waste!.items.length} 项值得优化`)
              : (wasteState === 'error' ? '识别失败' : wasteState === 'loading' ? 'LLM 识别中…' : '未分析')
          }
          accent={SAVE}
        />
        <Card
          label="Peak Context"
          value={fmtK(derived.peakVal)}
          sub={`step ${derived.peakIdx} · ${calls[derived.peakIdx].time}`}
          accent="#3b82f6"
        />
      </div>

      {/* 2 · 主图表 + 3 · Detail Panel */}
      <div className="grid gap-3 mb-4 items-start" style={{ gridTemplateColumns: '1fr minmax(240px, 288px)' }}>
        <div className="bg-white rounded-lg shadow border border-gray-200 p-3.5 overflow-hidden">
          <div className="flex justify-between items-baseline mb-2 flex-wrap gap-2">
            <h3 className="text-sm font-semibold text-gray-700 m-0">
              Context Window 组成 · 逐步重放 <span className="font-normal text-gray-400">· 点击柱体查看该步</span>
            </h3>
            {/* 图例 */}
            <div className="flex gap-3 flex-wrap">
              {visCtx.map(cat => (
                <span key={cat.key} className="inline-flex items-center gap-1.5 text-[11px] text-gray-500">
                  <i className="w-[9px] h-[9px] rounded-sm" style={{ background: cat.color }} />{cat.label}
                </span>
              ))}
              <span className="inline-flex items-center gap-1.5 text-[11px]" style={{ color: OUTPUT_COLOR }}>
                <i className="w-3.5 h-[2.5px] rounded-sm" style={{ background: OUTPUT_COLOR }} />输出 token
              </span>
            </div>
          </div>

          <div className="flex items-stretch">
            {/* 固定左栏：标注上/下两区语义（不随横向滚动移动） */}
            <div className="relative w-[22px] flex-shrink-0" style={{ height: svgH }}>
              <div className="absolute left-0 w-[22px] flex items-center justify-center" style={{ top: H_TOP, height: H_UP }}>
                <span className="font-mono text-[10px] tracking-widest text-gray-500 whitespace-nowrap" style={{ writingMode: 'vertical-rl' }}>已发送 · context</span>
              </div>
              <div className="absolute left-0 w-[22px] flex items-center justify-center" style={{ top: baseY, height: H_LOW }}>
                <span className="font-mono text-[10px] tracking-widest whitespace-nowrap" style={{ writingMode: 'vertical-rl', color: SAVE }}>可优化 · 可省</span>
              </div>
            </div>
            <div ref={containerRef} className="overflow-x-auto pb-1 flex-1">
              <svg viewBox={`0 0 ${chartW} ${svgH}`} width={chartW} height={svgH} className="block" style={{ maxWidth: 'none' }}>
                {/* 可优化 / 节省 区带（基线下方，绿浅底，区别于 context 区） */}
                {visOpt.length > 0 && (
                  <rect x={ML} y={baseY} width={chartW - 8 - ML} height={H_LOW} fill="rgba(16,185,129,.05)" />
                )}
                {/* y 轴刻度（上柱） */}
                {[0, 0.25, 0.5, 0.75, 1].map(f => {
                  const y = baseY - f * H_UP;
                  return (
                    <g key={f}>
                      <line x1={ML} y1={y} x2={chartW - 8} y2={y} stroke={GRID} strokeWidth={0.6} strokeDasharray={f === 0 ? '' : '2 3'} />
                      <text x={ML - 6} y={y + 3} textAnchor="end" fontSize={9} fill={AXIS} fontFamily="ui-monospace, monospace">{fmtK(maxCtx * f)}</text>
                    </g>
                  );
                })}
                {/* baseline 标注 */}
                <text x={ML - 6} y={baseY + 3} textAnchor="end" fontSize={9} fill="#6b7280" fontFamily="ui-monospace, monospace">0</text>

                {calls.map((c, i) => {
                  const x = ML + i * SLOT + (SLOT - BARW) / 2;
                  const isSel = i === selected;
                  // 上柱：从 baseline 向上堆叠
                  let yUp = baseY;
                  const upSegs = visCtx.map(cat => {
                    const v = cat.get(c);
                    const h = v * scale;
                    yUp -= h;
                    return { cat, v, y: yUp, h };
                  });
                  const barTop = yUp;
                  // 下柱：从 baseline 向下堆叠
                  let yLow = baseY;
                  const lowSegs = visOpt.map(cat => {
                    const v = cat.get(c);
                    const h = v * scale;
                    const seg = { cat, v, y: yLow, h };
                    yLow += h;
                    return seg;
                  });
                  return (
                    <g key={i} onClick={() => setSelected(i)} style={{ cursor: 'pointer' }} opacity={isSel || selected < 0 ? 1 : 0.82}>
                      {/* 选中背景高亮 */}
                      {isSel && <rect x={ML + i * SLOT} y={2} width={SLOT} height={svgH - H_X + 4} fill="rgba(59,130,246,.08)" />}
                      {/* 上柱段 */}
                      {upSegs.map((s, k) => s.h > 0 && (
                        <rect key={k} x={x} y={s.y} width={BARW} height={s.h} fill={s.cat.color} opacity={isSel ? 1 : 0.9} />
                      ))}
                      {/* 编排层：可省此轮 描边（虚线，建议·需确认） */}
                      {c.removable_turn && (
                        <rect x={x - 1.5} y={barTop - 1.5} width={BARW + 3} height={baseY - barTop + 3} fill="none" stroke={ROSE} strokeWidth={1.4} strokeDasharray="3 2" />
                      )}
                      {/* 下柱段（镜像） */}
                      {lowSegs.map((s, k) => s.h > 0 && (
                        <rect key={k} x={x} y={s.y} width={BARW} height={s.h} fill={s.cat.color} opacity={isSel ? 0.95 : 0.72} />
                      ))}
                      {/* x 轴标签（稀疏） */}
                      {(i % Math.ceil(n / 24 || 1) === 0 || isSel) && (
                        <text x={x + BARW / 2} y={svgH - 12} textAnchor="middle" fontSize={8} fill={isSel ? '#111827' : AXIS} fontFamily="ui-monospace, monospace">{i}</text>
                      )}
                    </g>
                  );
                })}
                {/* 下柱区图例 & 基线分隔 */}
                <line x1={ML} y1={baseY} x2={chartW - 8} y2={baseY} stroke="#6b7280" strokeWidth={1} />
                {visOpt.length > 0 && (
                  <text x={ML - 6} y={baseY + H_LOW + 3} textAnchor="end" fontSize={8} fill={AXIS} fontFamily="ui-monospace, monospace">-{fmtK(maxOpt)}</text>
                )}
                {/* 输出 token 曲线（叠加在柱状图上方，独立缩放） */}
                {n >= 2 && (() => {
                  const outScale = (H_UP * 0.85) / maxOut;
                  const pts = calls.map((c, i) => {
                    const cx = ML + i * SLOT + SLOT / 2;
                    const cy = baseY - c.output_tokens * outScale;
                    return `${cx},${cy}`;
                  }).join(' ');
                  return (
                    <g>
                      <polyline points={pts} fill="none" stroke={OUTPUT_COLOR} strokeWidth={1.6} strokeLinejoin="round" strokeLinecap="round" opacity={0.85} />
                      {calls.map((c, i) => {
                        const cx = ML + i * SLOT + SLOT / 2;
                        const cy = baseY - c.output_tokens * outScale;
                        return <circle key={i} cx={cx} cy={cy} r={i === selected ? 3.5 : 2} fill={OUTPUT_COLOR} stroke="#fff" strokeWidth={i === selected ? 1.2 : 0.6} opacity={i === selected ? 1 : 0.7} />;
                      })}
                    </g>
                  );
                })()}
              </svg>
            </div>
          </div>

          {/* 下柱图例 */}
          {visOpt.length > 0 && (
            <div className="flex gap-3 flex-wrap mt-2 pt-2 border-t border-gray-200">
              <span className="text-[10px] text-gray-500 font-mono tracking-wider">可优化量 ▼</span>
              {visOpt.map(cat => (
                <span key={cat.key} className="inline-flex items-center gap-1.5 text-[11px] text-gray-500">
                  <i className="w-[9px] h-[9px] rounded-sm" style={{ background: cat.color }} />{cat.label}
                </span>
              ))}
              <span className="inline-flex items-center gap-1.5 text-[11px]" style={{ color: ROSE }}>
                <i className="w-[9px] h-[9px] rounded-sm" style={{ border: `1.4px dashed ${ROSE}` }} />⚡可省此轮 · 需确认
              </span>
            </div>
          )}
        </div>

        {/* 3 · Detail Panel */}
        <DetailPanel c={sel} visCtx={visCtx} visOpt={visOpt} />
      </div>

      {/* 4 · 浪费诊断表（Rust 供数 → LLM 判定是否值得优化）*/}
      <WasteTable waste={waste} wasteState={wasteState} />
    </div>
  );
}

// ── 浪费诊断表 ──────────────────────────────────────────────────────────────────
function wasteColor(optimization: string): string {
  const o = optimization;
  if (o.includes('缓存') || o.includes('Cache')) return '#6366f1';
  if (o.includes('历史') || o.includes('History')) return '#14b8a6';
  if (o.includes('工具输出') || o.includes('Trim')) return '#eab308';
  if (o.includes('注入') || o.includes('Context')) return '#fb923c';
  if (o.includes('提示词压缩') || o.includes('Compression')) return '#3b82f6';
  return ROSE; // 步骤冗余 / 经验沉淀
}

function WasteTable({ waste, wasteState }: { waste?: WasteReport | null; wasteState?: DimState }) {
  // 加载中 / 失败 / 未分析态
  if (wasteState !== 'done' || !waste) {
    return (
      <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
        <h3 className="text-sm font-semibold text-gray-700">浪费诊断 · LLM 判定</h3>
        {wasteState === 'error' ? (
          <p className="text-red-500 font-mono text-[13px] mt-2">识别失败 —— LLM 调用出错，可稍后重试。</p>
        ) : wasteState === 'loading' ? (
          <div className="flex items-center gap-3 py-2 mt-1">
            <Spinner />
            <span className="text-gray-500 font-mono text-[13px]">LLM 正在逐条判定候选是否值得优化…</span>
          </div>
        ) : (
          <p className="text-gray-400 font-mono text-[13px] mt-2">尚未分析 —— 点击「重新分析」运行 LLM 浪费判定。</p>
        )}
      </div>
    );
  }

  if (waste.items.length === 0) {
    return (
      <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
        <h3 className="text-sm font-semibold text-gray-700">浪费诊断 · LLM 判定</h3>
        <p className="font-mono text-[13px] mt-2" style={{ color: SAVE }}>
          ✓ LLM 评估了 {waste.considered} 项候选，未发现值得优化的浪费。
        </p>
      </div>
    );
  }

  return (
    <div className="bg-white rounded-lg shadow border border-gray-200 p-5">
      <div className="flex items-baseline justify-between flex-wrap gap-2">
        <h3 className="text-sm font-semibold text-gray-700 m-0">浪费诊断 · LLM 判定</h3>
        <span className="text-[11px] text-gray-400 font-mono">
          {waste.items.length} 项值得优化 · {waste.dismissed} 项判为不值得 · 共评估 {waste.considered} 项
        </span>
      </div>
      <table className="w-full mt-3.5 table-fixed text-sm">
        <thead>
          <tr className="border-b border-gray-200">
            <th className="text-left pr-3 pb-2 text-xs font-semibold text-gray-600 w-[18%]">现象</th>
            <th className="text-left pr-3 pb-2 text-xs font-semibold text-gray-600 w-[18%]">浪费类型</th>
            <th className="text-left pr-3 pb-2 text-xs font-semibold text-gray-600">证据</th>
            <th className="text-left pr-3 pb-2 text-xs font-semibold text-gray-600 w-[16%]">优化手段</th>
            <th className="text-right pb-2 text-xs font-semibold text-gray-600 w-[14%]">预计可省</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-100">
          {waste.items.map((it, i) => (
            <tr key={i}>
              <td className="text-left pr-3 py-2 text-gray-700">{it.symptom}</td>
              <td className="text-left pr-3 py-2">
                <span className="inline-flex items-center gap-1.5 whitespace-nowrap">
                  <i className="w-[9px] h-[9px] rounded-sm flex-shrink-0" style={{ background: wasteColor(it.optimization) }} />
                  <span className="font-mono text-xs text-gray-700">{it.subtype}</span>
                  <span className="text-[10px] text-gray-400">· {it.confidence}</span>
                </span>
              </td>
              <td className="text-[11px] font-mono text-gray-500 text-left pr-3 py-2 overflow-hidden text-ellipsis whitespace-nowrap" title={it.evidence}>{it.evidence}</td>
              <td className="text-xs text-gray-700 text-left pr-3 py-2">{it.optimization}</td>
              <td className="text-right font-mono text-xs whitespace-nowrap py-2" style={{ color: it.needs_confirm ? '#6b7280' : SAVE }}>
                {fmtK(it.save_tokens)} tok
                {it.discount && <span className="text-gray-400"> · 折扣</span>}
                {it.needs_confirm && <span style={{ color: ROSE }}> · 需确认</span>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-[11px] text-gray-400 mt-3 font-mono">
        「是否值得优化」由 LLM 判定；token/金额由 Rust 按重放口径 × LLM 给的可裁剪比例估算。「步骤冗余」类属编排层建议，需人工确认。
      </p>
    </div>
  );
}

// ── Detail Panel ──────────────────────────────────────────────────────────────
function DetailPanel({ c, visCtx, visOpt }: {
  c: LLMCall; visCtx: Cat[]; visOpt: Cat[];
}) {
  const total = ctxTotal(c);
  const suggestions: string[] = [];
  if (c.cacheable > 200) suggestions.push('静态区域较大，可整块 Prefix Cache，节省固定成本约 80–95%。');
  if (c.history_prunable > 0) suggestions.push(`旧助手输出已累积，可裁剪/摘要约 ${fmtK(c.history_prunable)} tok。`);
  if (c.trimmable > 0) suggestions.push(`存在超长工具输出，可截断约 ${fmtK(c.trimmable)} tok。`);
  if (c.prunable > 0) suggestions.push(`低相关注入内容可裁剪约 ${fmtK(c.prunable)} tok。`);
  if (suggestions.length === 0 && !c.removable_turn) suggestions.push('该步无明显可优化项。');

  return (
    <div className="bg-white rounded-lg shadow border border-gray-200 p-4 sticky top-3">
      <div className="flex justify-between items-baseline">
        <h3 className="text-sm font-semibold text-gray-900 m-0">Step #{c.step_id}</h3>
        <span className="font-mono text-[11px] text-gray-500">{c.time} · {c.label}</span>
      </div>

      {/* 各类别 token + 百分比 */}
      <div className="mt-3.5 flex flex-col gap-[7px]">
        {visCtx.map(cat => {
          const v = cat.get(c);
          const pct = total > 0 ? (v / total) * 100 : 0;
          return (
            <div key={cat.key} className="grid items-center gap-2 text-xs" style={{ gridTemplateColumns: '10px 1fr auto' }}>
              <i className="w-[9px] h-[9px] rounded-sm" style={{ background: cat.color }} />
              <span className="text-gray-700">{cat.label}</span>
              <span className="font-mono text-[11px] text-gray-500">{fmtK(v)} · {pct.toFixed(0)}%</span>
            </div>
          );
        })}
        <div className="grid items-center gap-2 text-xs border-t border-gray-200 pt-1.5 mt-0.5" style={{ gridTemplateColumns: '10px 1fr auto' }}>
          <i className="w-[9px] h-[9px] rounded-sm" style={{ background: OUTPUT_COLOR }} />
          <span className="text-gray-700">本轮输出</span>
          <span className="font-mono text-[11px]" style={{ color: OUTPUT_COLOR }}>↑{fmtK(c.output_tokens)}</span>
        </div>
      </div>

      <div className="mt-3 text-[11px] font-mono text-gray-500">
        prompt_tokens ≈ <b className="text-gray-900">{fmtK(total)}</b>
      </div>

      {/* Optimization Potential */}
      {optTotal(c) > 0 && (
        <div className="mt-3.5 p-3 bg-gray-50 rounded border border-gray-200">
          <div className="font-mono text-[10px] tracking-widest uppercase mb-2" style={{ color: SAVE }}>Optimization Potential</div>
          {visOpt.filter(cat => cat.get(c) > 0).map(cat => (
            <div key={cat.key} className="grid items-center gap-2 text-xs mb-[5px]" style={{ gridTemplateColumns: '10px 1fr auto' }}>
              <i className="w-[9px] h-[9px] rounded-sm" style={{ background: cat.color }} />
              <span className="text-gray-700">{cat.label}</span>
              <span className="font-mono text-[11px] text-gray-500">{fmtK(cat.get(c))} tok</span>
            </div>
          ))}
        </div>
      )}

      {/* 编排层建议 */}
      {c.removable_turn && (
        <div className="mt-3 p-3 bg-pink-50 rounded border border-dashed" style={{ borderColor: ROSE }}>
          <div className="text-xs text-gray-900">⚡ <b>可省此轮</b></div>
          <div className="text-[11px] text-gray-500 mt-1">疑似冗余重试 / 空转轮，可合并或消除。<b style={{ color: ROSE }}>建议 · 需人工确认</b></div>
        </div>
      )}

      {/* 动态 Suggestions */}
      <div className="mt-3">
        <div className="font-mono text-[10px] tracking-widest uppercase text-gray-500 mb-1.5">Suggestions</div>
        <ul className="m-0 pl-4 flex flex-col gap-[5px] list-disc">
          {suggestions.map((s, i) => <li key={i} className="text-xs text-gray-500">{s}</li>)}
        </ul>
      </div>
    </div>
  );
}
