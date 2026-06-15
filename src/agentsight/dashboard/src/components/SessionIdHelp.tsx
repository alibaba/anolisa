import React, { useEffect, useRef, useState } from 'react';

/**
 * Session ID 用法说明小图标。
 *
 * 设计要点：
 *  - 自定义 tooltip 而非原生 `title`，原生要等约 1 秒才弹出，这里立即响应。
 *  - tooltip 用 `position: fixed` + `getBoundingClientRect` 定位，逃出表格父容器
 *    `overflow-hidden` 的裁剪。
 *  - 鼠标从 `?` 移出后给 100ms 宽限期允许进入卡片本身；进入卡片时取消计时器，
 *    保持开启；从卡片完全离开后才真正关闭，避免抖动闪烁。
 *  - 仅承载「说明」语义，不承载「跳转」——使用入口由顶部 NavBar 的
 *    「🔍 ATIF 查看器」承担，避免一个 `?` 同时背负两种不一致的点击语义。
 *  - 组件卸载时清理未触发的 setTimeout，防止 React unmounted-component setState
 *    告警。
 */
export const SessionIdHelp: React.FC = () => {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ top: number; left: number }>({ top: 0, left: 0 });
  const anchorRef = useRef<HTMLSpanElement>(null);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cancelHide = () => {
    if (hideTimerRef.current) {
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
  };

  const show = () => {
    cancelHide();
    const el = anchorRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({ top: r.bottom + 6, left: r.left });
    setOpen(true);
  };

  const scheduleHide = () => {
    cancelHide();
    hideTimerRef.current = setTimeout(() => setOpen(false), 100);
  };

  // 卸载时清理可能挂起的关闭计时器，避免 setState-after-unmount 告警。
  useEffect(() => {
    return () => {
      if (hideTimerRef.current) {
        clearTimeout(hideTimerRef.current);
        hideTimerRef.current = null;
      }
    };
  }, []);

  return (
    <>
      <span
        ref={anchorRef}
        role="img"
        aria-label="Session ID 用法说明"
        tabIndex={0}
        onMouseEnter={show}
        onMouseLeave={scheduleHide}
        onFocus={show}
        onBlur={scheduleHide}
        className="inline-flex items-center justify-center w-4 h-4 rounded-full bg-gray-200 hover:bg-gray-300 text-gray-500 text-[10px] font-bold normal-case tracking-normal align-middle select-none"
      >
        ?
      </span>
      {open && (
        <div
          role="tooltip"
          onMouseEnter={cancelHide}
          onMouseLeave={scheduleHide}
          style={{ top: pos.top, left: pos.left, position: 'fixed' }}
          className="z-50 w-72 rounded-md bg-gray-900 text-white text-[11px] leading-relaxed normal-case tracking-normal px-3 py-2 shadow-lg"
        >
          <div className="font-semibold text-blue-200 mb-1">Session ID 用法</div>
          <div>唯一标识一次 Agent 会话。</div>
          <div className="mt-1.5">用途：</div>
          <div>① 排查问题时在日志里过滤会话</div>
          <div>② 通过 agentsight CLI / API 检索会话详情</div>
          <div>③ 在「🔍 ATIF 查看器」页面粘入 ID 查看完整 trace</div>
          <div className="mt-1.5 text-blue-200">
            点击右侧「复制」后，可在顶部导航栏「🔍 ATIF 查看器」中粘贴查询。
          </div>
        </div>
      )}
    </>
  );
};
