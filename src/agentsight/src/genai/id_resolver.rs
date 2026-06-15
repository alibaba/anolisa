//! Session / Conversation ID resolver.
//!
//! 将 `session_id` 的 fallback 与 `conversation_id` 的计算改造为：
//!
//! ```text
//! session_id      = SHA256("session"      + 该 session  内最早 response_id)[..32]
//! conversation_id = SHA256("conversation" + 该 conv     内最早 response_id)[..32]
//! ```
//!
//! 同一 session / conversation 的"最早 response_id"通过两个 LRU 缓存锚定：
//! - `session_first_resp`：以 `(agent_name, pid, 首条 user message)` 的 SHA256 作为 key
//! - `conv_first_resp`   ：以 `(agent_name, pid, 最后一条 user message)` 的 SHA256 作为 key
//!
//! 在 key 中加入 `agent_name + pid` 维度是为了避免同机不同 agent（或
//! 同 agent 不同进程）在用户输入相同时撞到同一 LRU bucket 从而获得
//! 相同的 session_id / conversation_id。
//!
//! 加域前缀（"session" / "conversation"）实现域分离，确保两类 ID 不会
//! 在 `first_response_id` 相同的边角情况下发生哈希碰撞。
//!
//! 当传入的 `response_id` 为空、或 user message 文本为空时直接返回 `None`，
//! 由调用方决定是否回写元数据；后续 `complete_pending` 阶段会带着真正的
//! response_id 再次调用本模块完成回填。

use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;
use sha2::{Digest, Sha256};

/// Maximum number of (session_key | conv_key) → first_response_id entries
/// kept in memory. Sized to align with `ResponseSessionMapper`.
const MAX_ENTRIES: usize = 10_000;

/// 域前缀：避免 session_id 与 conversation_id 在 first_response_id 相同时碰撞。
const SESSION_DOMAIN: &str = "session";
const CONVERSATION_DOMAIN: &str = "conversation";

/// `IdResolver` 负责把"内容 key + 当前 response_id"折算为稳定的
/// session_id / conversation_id。
///
/// 内部使用 `Mutex<LruCache<...>>`，所以可以以 `&self` 形式安全地在
/// `GenAIBuilder` 多线程上下文里调用。
pub struct IdResolver {
    /// session_key (SHA256 of first user message) → first_response_id.
    session_first_resp: Mutex<LruCache<String, String>>,
    /// conversation_key (SHA256 of last user message) → first_response_id.
    conv_first_resp: Mutex<LruCache<String, String>>,
}

impl Default for IdResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl IdResolver {
    /// 构造一个空的 resolver，两份 LRU 容量均为 [`MAX_ENTRIES`]。
    pub fn new() -> Self {
        let cap = NonZeroUsize::new(MAX_ENTRIES).expect("MAX_ENTRIES must be non-zero");
        IdResolver {
            session_first_resp: Mutex::new(LruCache::new(cap)),
            conv_first_resp: Mutex::new(LruCache::new(cap)),
        }
    }

    /// 计算 session_id。
    ///
    /// - `agent_name`：该调用所属的 agent 名称（OpenClaw / Cosh / Hermes / ...）。
    ///   与 `pid` 一起加入 LRU key，避免同机不同 agent / 不同进程在 user query
    ///   相同时撞库。传空串也合法，会作为 key 的一部分参与哈希。
    /// - `pid`：产生本次调用的进程 ID。重启后 PID 变化会自然产生新
    ///   session_id，这与"一个 agent 进程一个会话"的语义一致。
    /// - `first_user_text`：当前请求中"第一条 user message"的原始文本。
    ///   空字符串视为无法定位 session_key，返回 `None`。
    /// - `response_id`：当前 LLM 调用的 response_id；空字符串返回 `None`。
    ///
    /// 多次调用同一 `(agent_name, pid, first_user_text)` 时锚定第一次写入的
    /// `response_id`，后续 `response_id` 即使变化，也会得到与首次相同的
    /// session_id。
    pub fn resolve_session_id(
        &self,
        agent_name: &str,
        pid: i32,
        first_user_text: &str,
        response_id: &str,
    ) -> Option<String> {
        Self::resolve(
            &self.session_first_resp,
            SESSION_DOMAIN,
            agent_name,
            pid,
            first_user_text,
            response_id,
        )
    }

    /// 计算 conversation_id。
    ///
    /// - `agent_name` / `pid`：同 `resolve_session_id`，用于隔离同机不同 agent /
    ///   不同进程。
    /// - `last_user_text`：当前请求中"最后一条 user message"的原始文本，
    ///   作为 conversation_key 的素材。空字符串返回 `None`。
    /// - `response_id`：当前调用的 response_id；空字符串返回 `None`。
    pub fn resolve_conversation_id(
        &self,
        agent_name: &str,
        pid: i32,
        last_user_text: &str,
        response_id: &str,
    ) -> Option<String> {
        Self::resolve(
            &self.conv_first_resp,
            CONVERSATION_DOMAIN,
            agent_name,
            pid,
            last_user_text,
            response_id,
        )
    }

    /// 通用实现：定位/记录"首个 response_id"，再用域前缀 + first_response_id
    /// 计算最终 ID（取 SHA256 前 32 位 hex）。
    fn resolve(
        cache: &Mutex<LruCache<String, String>>,
        domain: &str,
        agent_name: &str,
        pid: i32,
        text: &str,
        response_id: &str,
    ) -> Option<String> {
        if text.is_empty() || response_id.is_empty() {
            return None;
        }

        let key = compose_key(agent_name, pid, text);
        let first_response_id = {
            let mut guard = cache.lock().expect("IdResolver LRU mutex poisoned");
            // 已有条目时直接复用首个 response_id；否则把当前 response_id
            // 写入作为锚点，让同一 key 后续调用得到稳定结果。
            if let Some(existing) = guard.get(&key) {
                existing.clone()
            } else {
                guard.put(key, response_id.to_string());
                response_id.to_string()
            }
        };

        Some(domain_hash(domain, &first_response_id))
    }

    /// 只读查询 session_id：查 LRU 中是否已锁定 `(agent, pid, first_user_text)` 的
    /// first_response_id，命中则返回与正常路径完全一致的 session_id，未命中
    /// 返回 `None`（不写入、不修改 LRU 顺序）。
    ///
    /// 主要用于 crash-drain 路径：进程崩溃、响应永远不会到达时，如果
    /// 同 PID 之前已正常完成过 LLM 调用（LRU 已 anchor），则复用同一个
    /// session_id，避免同一会话被崩溃拆分。
    pub fn peek_session_id(
        &self,
        agent_name: &str,
        pid: i32,
        first_user_text: &str,
    ) -> Option<String> {
        Self::peek(
            &self.session_first_resp,
            SESSION_DOMAIN,
            agent_name,
            pid,
            first_user_text,
        )
    }

    /// 只读查询 conversation_id，用途与 `peek_session_id` 同。
    pub fn peek_conversation_id(
        &self,
        agent_name: &str,
        pid: i32,
        last_user_text: &str,
    ) -> Option<String> {
        Self::peek(
            &self.conv_first_resp,
            CONVERSATION_DOMAIN,
            agent_name,
            pid,
            last_user_text,
        )
    }

    /// `peek_*` 的通用实现：仅查询 LRU，不写入、不提升顺序。
    fn peek(
        cache: &Mutex<LruCache<String, String>>,
        domain: &str,
        agent_name: &str,
        pid: i32,
        text: &str,
    ) -> Option<String> {
        if text.is_empty() {
            return None;
        }
        let key = compose_key(agent_name, pid, text);
        let guard = cache.lock().ok()?;
        // `LruCache::peek` 不会提升条目顺序，适合 crash 路径“只读不写”。
        let first_resp = guard.peek(&key)?;
        Some(domain_hash(domain, first_resp))
    }
}

/// crash-drain 路径专用 fallback ID。
///
/// 仅在 PID 第一个请求就崩、`peek_*` 未命中 LRU 时使用。调用方需传入：
/// - `domain`: "session" 或 "conversation"（函数内部会加 "crash-" 前缀做域隔离）
/// - `agent_name`/`pid`: 与正常路径 LRU key 维度一致，隔离同机不同 agent
/// - `user_text`: session 传 first_user_text；conversation 传 last_user_text，
///   跟正常路径“一个 user_query 一个 conversation”的语义对齐。
///
/// 输出 = `SHA256("crash-{domain}|agent_name|pid|user_text")[..32]`。
/// `crash-` 前缀与正常路径的 `session`/`conversation` 前缀做域分离，避免
/// crash 兑底 ID 与正常调用 ID 碰撞。
pub fn crash_fallback_id(domain: &str, agent_name: &str, pid: i32, user_text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("crash-{}", domain).as_bytes());
    hasher.update(b"|");
    hasher.update(agent_name.as_bytes());
    hasher.update(b"|");
    hasher.update(pid.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(user_text.as_bytes());
    let digest = hasher.finalize();
    let full = format!("{:x}", digest);
    full[..32].to_string()
}

/// 组装 LRU key：SHA256(agent_name + "|" + pid + "|" + text) 的 hex。
///
/// 使用哈希后的定长字符串作为 key，避免原始 user message 过长占用内存；
/// `|` 作为字段分隔符避免 "a"+"|b" 与 "a|"+"b" 这类拼接哈希冲突。
fn compose_key(agent_name: &str, pid: i32, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(agent_name.as_bytes());
    hasher.update(b"|");
    hasher.update(pid.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 计算 `SHA256(domain + first_response_id)` 的前 32 位 hex 表示。
fn domain_hash(domain: &str, first_response_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(first_response_id.as_bytes());
    let digest = hasher.finalize();
    let full = format!("{:x}", digest);
    full[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：默认 agent / pid。
    const A: &str = "openclaw";
    const P: i32 = 1001;

    #[test]
    fn resolve_session_id_is_stable_across_calls() {
        let resolver = IdResolver::new();
        let first = resolver
            .resolve_session_id(A, P, "user-A", "resp-1")
            .unwrap();
        // 即便后续 response_id 变了，同一 (agent, pid, first_user_text) 仍返回首次结果
        let again = resolver
            .resolve_session_id(A, P, "user-A", "resp-2")
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn resolve_session_id_changes_with_first_response_id() {
        // 不同 first_user_text 处于不同 LRU bucket，各自锁定首次看到的
        // response_id；只要首次看到的 response_id 不同，得到的 session_id
        // 就会不同。
        let resolver = IdResolver::new();
        let a = resolver
            .resolve_session_id(A, P, "user-A", "resp-1")
            .unwrap();
        let b = resolver
            .resolve_session_id(A, P, "user-B", "resp-2")
            .unwrap();
        assert_ne!(a, b, "不同首次 response_id 应产生不同 session_id");
    }

    #[test]
    fn resolve_conversation_id_changes_with_last_user_text() {
        let resolver = IdResolver::new();
        let a = resolver
            .resolve_conversation_id(A, P, "turn-1", "resp-1")
            .unwrap();
        let b = resolver
            .resolve_conversation_id(A, P, "turn-2", "resp-2")
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn resolve_returns_none_when_response_id_empty() {
        let resolver = IdResolver::new();
        assert!(resolver.resolve_session_id(A, P, "user-A", "").is_none());
        assert!(
            resolver
                .resolve_conversation_id(A, P, "turn-1", "")
                .is_none()
        );
    }

    #[test]
    fn resolve_returns_none_when_text_empty() {
        let resolver = IdResolver::new();
        assert!(resolver.resolve_session_id(A, P, "", "resp-1").is_none());
        assert!(
            resolver
                .resolve_conversation_id(A, P, "", "resp-1")
                .is_none()
        );
    }

    #[test]
    fn session_and_conversation_diverge_on_same_first_response_id() {
        let resolver = IdResolver::new();
        // 故意构造 session_key 与 conversation_key 相同（同一段文本既是首条
        // 又是最后一条 user message 时的真实场景），但仍应得到不同 ID。
        let text = "single-turn";
        let resp = "resp-1";
        let s = resolver.resolve_session_id(A, P, text, resp).unwrap();
        let c = resolver.resolve_conversation_id(A, P, text, resp).unwrap();
        assert_ne!(s, c, "域前缀应保证两类 ID 不会碰撞");
    }

    #[test]
    fn different_agents_with_same_user_text_get_different_ids() {
        // 同机不同 agent，用户输入完全相同。生产中两个 agent 的 LLM
        // 调用会各自拿到不同的 response_id，加入 (agent_name, pid) 作为 LRU
        // key 后两个调用会各自锁定自己的首个 response_id，不会被同机
        // 其他 agent “首访”串走。
        let resolver = IdResolver::new();
        let openclaw = resolver
            .resolve_session_id("openclaw", 1001, "今天天气", "chatcmpl-A")
            .unwrap();
        let cosh = resolver
            .resolve_session_id("cosh", 2002, "今天天气", "chatcmpl-B")
            .unwrap();
        assert_ne!(openclaw, cosh, "同机不同 agent 不可联合为同一 session");

        let openclaw_conv = resolver
            .resolve_conversation_id("openclaw", 1001, "今天天气", "chatcmpl-A")
            .unwrap();
        let cosh_conv = resolver
            .resolve_conversation_id("cosh", 2002, "今天天气", "chatcmpl-B")
            .unwrap();
        assert_ne!(openclaw_conv, cosh_conv);
    }

    #[test]
    fn same_user_text_does_not_leak_response_id_across_agents() {
        // 验证 LRU 分桶：同一 user_text，不同 agent 各自锁定自己首访的
        // response_id。即 OpenClaw 先到后，Cosh 后到不会被 OpenClaw 的
        // response_id “传染”。
        let resolver = IdResolver::new();
        let openclaw_first = resolver
            .resolve_session_id("openclaw", 1001, "hello", "chatcmpl-A")
            .unwrap();
        // Cosh 后到，拿到的应该是自己的 chatcmpl-B 作为 first_response_id，
        // 而不是复用 chatcmpl-A。
        let cosh_first = resolver
            .resolve_session_id("cosh", 2002, "hello", "chatcmpl-B")
            .unwrap();
        // OpenClaw 后续调用仍锁定 chatcmpl-A
        let openclaw_second = resolver
            .resolve_session_id("openclaw", 1001, "hello", "chatcmpl-X")
            .unwrap();
        assert_eq!(openclaw_first, openclaw_second, "OpenClaw 多轮调用应稳定");
        assert_ne!(openclaw_first, cosh_first, "不同 agent 不会撞库");
    }

    #[test]
    fn same_agent_different_pids_get_different_ids() {
        // 同 agent 两个进程实例（如重启后），同样 user query 应产生
        // 不同会话 ID，符合"一个进程 = 一个会话"语义。
        let resolver = IdResolver::new();
        let p1 = resolver
            .resolve_session_id("openclaw", 1001, "hello", "resp-1")
            .unwrap();
        let p2 = resolver
            .resolve_session_id("openclaw", 1002, "hello", "resp-2")
            .unwrap();
        assert_ne!(p1, p2);
    }

    // ── peek_* 只读查询接口测试 ──

    #[test]
    fn peek_session_id_returns_none_when_lru_empty() {
        let resolver = IdResolver::new();
        assert!(resolver.peek_session_id(A, P, "unseen").is_none());
        assert!(resolver.peek_conversation_id(A, P, "unseen").is_none());
    }

    #[test]
    fn peek_session_id_matches_resolve_when_anchored() {
        // 验证 crash 路径与正常路径自动对齐：resolve 写入后，peek 返回
        // 与 resolve 完全相同的 ID。
        let resolver = IdResolver::new();
        let normal = resolver
            .resolve_session_id(A, P, "hello", "chatcmpl-A")
            .unwrap();
        let peeked = resolver.peek_session_id(A, P, "hello").unwrap();
        assert_eq!(normal, peeked, "peek 应返回与正常路径一致的 session_id");

        let normal_conv = resolver
            .resolve_conversation_id(A, P, "world", "chatcmpl-B")
            .unwrap();
        let peeked_conv = resolver.peek_conversation_id(A, P, "world").unwrap();
        assert_eq!(normal_conv, peeked_conv);
    }

    #[test]
    fn peek_returns_none_when_text_empty() {
        let resolver = IdResolver::new();
        // 即使 LRU 中有条目，空 text 仍返回 None
        let _ = resolver.resolve_session_id(A, P, "x", "resp");
        assert!(resolver.peek_session_id(A, P, "").is_none());
        assert!(resolver.peek_conversation_id(A, P, "").is_none());
    }

    // ── crash_fallback_id 测试 ──

    #[test]
    fn crash_fallback_id_stable_for_same_inputs() {
        let a = crash_fallback_id("session", "openclaw", 1001, "hello");
        let b = crash_fallback_id("session", "openclaw", 1001, "hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn crash_fallback_id_diverges_session_vs_conversation() {
        let s = crash_fallback_id("session", "openclaw", 1001, "hello");
        let c = crash_fallback_id("conversation", "openclaw", 1001, "hello");
        assert_ne!(s, c, "同输入下 session/conversation 域返回不同值");
    }

    #[test]
    fn crash_fallback_id_diverges_with_different_user_text() {
        // 验证 user_query 粒度分桶：同 PID 同 agent，但不同 user_text
        // 产生不同的 crash fallback ID。
        let a = crash_fallback_id("session", "openclaw", 1001, "query-A");
        let b = crash_fallback_id("session", "openclaw", 1001, "query-B");
        assert_ne!(a, b);
    }

    #[test]
    fn crash_fallback_id_diverges_from_normal_id() {
        // 以同一 user_text 分别走正常路径与 crash fallback，两者应因为
        // 域前缀 ("session" vs "crash-session") 不同而不冲突。
        let resolver = IdResolver::new();
        let normal = resolver
            .resolve_session_id("openclaw", 1001, "hello", "chatcmpl-A")
            .unwrap();
        let crash = crash_fallback_id("session", "openclaw", 1001, "hello");
        assert_ne!(
            normal, crash,
            "正常 ID 与 crash fallback 需严格隔离以避免下游聚合错乱"
        );
    }
}
