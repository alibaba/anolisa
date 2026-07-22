#[macro_use]
mod startup;
#[macro_use]
mod help;
#[macro_use]
mod config;
#[macro_use]
mod hooks;
#[macro_use]
mod debug;
#[macro_use]
mod modes;
#[macro_use]
mod agent;
#[macro_use]
mod insight;
#[macro_use]
mod hook_details;
#[macro_use]
mod activity;
#[macro_use]
mod recommendation;
#[macro_use]
mod health;
#[macro_use]
mod question;
#[macro_use]
mod approval;
#[macro_use]
mod session;

macro_rules! define_message_id {
    ($($id:ident,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum MessageId {
            $($id,)*
        }

        impl MessageId {
            pub const ALL: &'static [MessageId] = &[
                $(MessageId::$id,)*
            ];
        }
    };
}

macro_rules! collect_message_ids {
    ([$segment:ident $(, $remaining:ident)* $(,)?], $($ids:ident,)*) => {
        $segment!(collect_message_ids, [$($remaining),*], $($ids,)*);
    };
    ([], $($ids:ident,)*) => {
        define_message_id!($($ids,)*);
    };
}

// Segment order preserves the public fieldless enum's existing discriminants.
collect_message_ids!([
    startup_ids,
    help_core_ids,
    config_ids,
    hooks_command_ids,
    debug_ids,
    legacy_approval_mode_ids,
    removed_command_ids,
    mode_ids,
    agent_ids,
    hook_insight_ids,
    agent_queue_ids,
    hook_details_ids,
    activity_ids,
    recommendation_ids,
    health_ids,
    tool_summary_ids,
    question_ids,
    approval_ids,
    help_session_ids,
    session_ids,
    help_registry_ids,
],);
