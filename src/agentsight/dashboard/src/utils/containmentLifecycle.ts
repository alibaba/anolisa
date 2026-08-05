import type { SecurityContainmentAction } from './apiClient';

export interface ContainmentLifecyclePresentation {
  label: string;
  detail: string;
  style: string;
}

export function containmentLifecyclePresentation(
  action: SecurityContainmentAction,
): ContainmentLifecyclePresentation {
  switch (action.lifecycle_state) {
    case 'pending':
      return { label: '等待执行', detail: '策略已提交，等待执行器确认', style: 'bg-amber-100 text-amber-700' };
    case 'active':
      return action.blocked_at_ns !== null
        ? { label: '已遏制', detail: '内核已确认阻断', style: 'bg-red-100 text-red-700' }
        : { label: '策略生效', detail: '等待首次内核阻断', style: 'bg-blue-100 text-blue-700' };
    case 'expiring':
      return { label: '正在解除', detail: '执行器正在清理策略', style: 'bg-amber-100 text-amber-700' };
    case 'expired':
      return { label: '已到期', detail: '临时策略已解除', style: 'bg-gray-100 text-gray-700' };
    case 'failed':
      return { label: '执行失败', detail: '可在确认运行状态后重新下发', style: 'bg-red-100 text-red-700' };
  }
}
