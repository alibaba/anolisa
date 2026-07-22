use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::SessionTitle => "智能体会话",
        MessageId::SessionUnavailableBody => "会话恢复需要使用 cosh-core 后端。",
        MessageId::SessionBusyBody => "请先结束当前智能体任务或交互面板，再管理会话。",
        MessageId::SessionEmptyBody => "当前工作区没有已持久化的会话。",
        MessageId::SessionListFooter => "使用 /session 打开选择器，或使用 /session resume <id>。",
        MessageId::SessionStatusTitle => "会话恢复状态",
        MessageId::SessionShellIdLine => "终端会话：{id}",
        MessageId::SessionProviderIdLine => "活动模型会话：{active}\n已选择模型会话：{selected}",
        MessageId::SessionWorkspaceLine => "工作区：{workspace}",
        MessageId::SessionRecoveryLine => "恢复状态：{state}",
        MessageId::SessionErrorLine => "最近恢复错误：[{code}] {error}",
        MessageId::SessionEvidenceNotRestoredBody => {
            "历史终端证据不会恢复；仅恢复模型可见的对话上下文。"
        }
        MessageId::SessionPickerFooter => {
            "上/下或 j/k 移动 · Enter 恢复 · Space 标记清理 · d 清理 · Esc 取消"
        }
        MessageId::SessionClearConfirmTitle => "确认清理会话",
        MessageId::SessionClearConfirmCountLine => "将删除以下 {count} 个持久化会话：",
        MessageId::SessionClearConfirmFooter => "Enter 或 y 确认 · Esc、Ctrl+C 或 n 取消",
        MessageId::SessionSelectedTitle => "已选择会话",
        MessageId::SessionSelectedBody => "下次智能体请求将恢复模型会话 {id}。",
        MessageId::SessionErrorTitle => "会话恢复",
        MessageId::SessionClearedTitle => "会话已清理",
        MessageId::SessionClearedBody => "已删除 {count} 个持久化会话。",
        MessageId::SessionSkippedBody => "已跳过 {count} 个受保护或不可用的会话。",
        MessageId::SessionClearInterruptedBody => {
            "清理中断 [{code}]：{unknown} 个会话状态未知，{unattempted} 个尚未尝试。"
        }
        MessageId::SessionCancelledTitle => "会话管理已关闭",
        MessageId::SessionCancelledBody => "模型会话和持久化文件均未改变。",
        MessageId::SessionUsageBody => {
            "用法：/session [status|list|resume <id>|clear <id>...|clear --all]"
        }
        MessageId::SessionNotReadyBody => "会话 {id} 的状态为 {health}，无法恢复，但仍可清理。",
        MessageId::SessionProtectedBody => "活动中或已选择的模型会话受保护，未被清理。",
        _ => return None,
    })
}
