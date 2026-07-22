macro_rules! question_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            QuestionTitle,
            QuestionDefaultPrompt,
            QuestionAnswerLabel,
            QuestionSelectOneLabel,
            QuestionSelectMultipleLabel,
            QuestionOtherEmptyLabel,
            QuestionKeysPrefix,
            QuestionInstructionMoveTypeSend,
            QuestionInstructionMoveToggleSend,
            QuestionInstructionMoveSend,
            QuestionInstructionTypeSend,
            QuestionInstructionNoAnswer,
            QuestionNoPendingTitle,
            QuestionNoPendingBody,
        );
    };
}
