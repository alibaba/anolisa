use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::QuestionTitle => "Agent 问题",
        MessageId::QuestionDefaultPrompt => "Agent 需要你的输入",
        MessageId::QuestionAnswerLabel => "回答",
        MessageId::QuestionSelectOneLabel => "选择一项:",
        MessageId::QuestionSelectMultipleLabel => "选择一项或多项:",
        MessageId::QuestionOtherEmptyLabel => "其他...",
        MessageId::QuestionKeysPrefix => "按键: ",
        MessageId::QuestionInstructionMoveTypeSend => "左/右移动 | 输入回答 | Enter 发送",
        MessageId::QuestionInstructionMoveToggleSend => "左/右移动 | Space 切换 | Enter 发送",
        MessageId::QuestionInstructionMoveSend => "左/右移动 | Enter 发送",
        MessageId::QuestionInstructionTypeSend => "输入回答 | Enter 发送",
        MessageId::QuestionInstructionNoAnswer => "没有可选择的回答。",
        MessageId::QuestionNoPendingTitle => "没有待回答问题",
        MessageId::QuestionNoPendingBody => "当前没有等待回答的 Agent 问题。",
        _ => return None,
    })
}
