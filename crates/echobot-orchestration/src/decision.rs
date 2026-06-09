//! `DecisionEngine` and the regex rules that route a turn to chat or agent.
//!
//! Mirrors `echobot/orchestration/decision.py`. The regex patterns are
//! kept verbatim from the Python source. The decision flow is:
//!
//! 1. `chat_only` / `force_agent` route modes short-circuit.
//! 2. The regex rules in [`AGENT_PATTERNS`] classify "obvious" agent work.
//! 3. Otherwise the supplied [`DeciderAgent`] (a one-shot LLM) is asked
//!    for a JSON decision. Falls back to `chat` if everything is
//!    unavailable or the response is unparseable.

use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use tracing::warn;

use echobot_core::models::{message_content_to_text, LLMMessage};

use crate::route_modes::{normalize_route_mode, RouteMode};

/// The system prompt given to the lightweight decider LLM. Verbatim port
/// of the Python `DECISION_SYSTEM_PROMPT`.
pub const DECISION_SYSTEM_PROMPT: &str = "\
You are the decision layer for a three-layer real-time assistant.

Your job is to choose the fastest safe route for the current turn.

Choose one route:
- \"chat\": a direct conversational reply is enough. Use this for casual conversation, roleplay, opinions, brainstorming, rewriting, translation, emotional support, or general discussion that can be answered from the current message plus recent chat context alone.
- \"agent\": the assistant must inspect, change, search, verify, schedule, or execute anything beyond the current visible chat reply.

Always choose \"agent\" for:
- Any tool use, project or file inspection, code review or edits, shell commands, skill use, or background work.
- Any memory lookup, including \"do you remember...\" questions about user preferences, prior topics, previous tasks, or earlier decisions.
- Any scheduling, reminders, cron, heartbeat, timers, or checking existing scheduled jobs.
- Any request that depends on external state such as workspace files, saved memory, schedule state, prior tool output, or background job status.
- Any follow-up that modifies, continues, retries, or asks about an earlier actionable task, for example \"do that\", \"continue\", \"try again\", \"change it to tomorrow at 9\", or \"what was the result?\"

Choose \"chat\" only when no lookup, tool call, memory search, scheduling action, or workspace inspection is needed.

If the request is ambiguous, prefer \"agent\" when there is a meaningful chance the user is referring to prior agent work or stored state. Otherwise prefer \"chat\".

Return JSON only:
{\"route\":\"chat\"|\"agent\",\"reason\":\"short reason\"}";

/// Default cap on tokens for the decider LLM call.
pub const DEFAULT_DECISION_MAX_TOKENS: u32 = 4096;

/// English politeness prefix used by every English agent-detection pattern.
/// Verbatim port of the Python constant.
pub const ENGLISH_REQUEST_PREFIX: &str = r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?";

/// Chinese politeness prefix used by every Chinese agent-detection pattern.
/// Verbatim port of the Python constant.
pub const CHINESE_REQUEST_PREFIX: &str = r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*";

/// The agent-detection patterns. Each entry is the raw regex pattern
/// (no `(?i)`; Python's `re.IGNORECASE` is applied at match time).
///
/// Verbatim port of the Python `AGENT_PATTERNS` tuple.
pub const AGENT_PATTERNS: &[&str] = &[
    // scheduling and background execution
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:set|create|add|schedule)\s+(a\s+)?(cron|reminder|timer|task)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:start|stop|enable|disable|check|show)\s+(the\s+)?(cron|heartbeat)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:run in the background|background task)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?remind\s+me\s+to\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:set\s+(a\s+)?(reminder|timer)|schedule\s+(a\s+)?(reminder|task))\b.*\b(in|after)\s+\d+\s*(seconds?|minutes?|hours?|days?)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?\b(in|after)\s+\d+\s*(seconds?|minutes?|hours?|days?)\b.*\b(remind\s+me\s+to|set\s+(a\s+)?(reminder|timer)|schedule\s+(a\s+)?(reminder|task))\b",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:设置|创建|添加|安排).*(cron|提醒|定时|计划任务)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:开启|关闭|启动|停止|检查|查看).*(心跳|cron)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:提醒我.*(后|在|每|去|做)|设置提醒|定时提醒|计划任务|后台任务|后台执行)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*\d+\s*(秒|分钟|小时|天)后.*提醒我",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:每[天周月年]).*(提醒|执行|运行)",
    // workspace, files, code, tools, and memory operations
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:open|read|view|inspect|check)\s+(the\s+)?(file|files|folder|directory|repo|repository|project|workspace|codebase)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:open|read|view|inspect|edit|modify|delete)\s+\S+\.\w+\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:search|scan|inspect|check)\s+(the\s+)?(repo|repository|project|workspace|codebase)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:search|find|look\s+up)\s+(in|through)\s+(the\s+)?(file|files|code|repo|repository|project|codebase|directory|workspace|memory|memories)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:edit|modify|delete|remove|rename|move)\s+(the\s+|a\s+)?(file|files|folder|directory|repo|repository|project|workspace|code|script|function|class|module|test|command|program)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:create|write|generate)\s+(a\s+|the\s+)?(file|script|function|class|module|test|command|program)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:run|execute)\s+(a\s+|the\s+)?(script|command|test|program|process|shell command|terminal command)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:use|activate|install|list)\s+(a\s+|the\s+)?skills?\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:use|run|call|list)\s+(a\s+|the\s+)?tools?\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:remember|save|store|log)\s+(this|that|it)\b",
    r"^\s*(?:(?:please|kindly)\s+)?(?:(?:can|could|would)\s+you\s+)?(?:help\s+me\s+)?(?:look up|search|check|recall)\s+(the\s+)?memories?\b",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:打开|查看|读取|检查|搜索|查找).*(文件|代码|项目|仓库|目录|工作区|记忆)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:修改|编辑|删除|移除|重命名|移动).*(文件|代码|脚本|函数|类|模块|测试|命令|目录|项目)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:创建|新建|生成|写).*(文件|脚本|函数|类|模块|测试|命令|程序)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:运行|执行).*(脚本|命令|测试|程序|进程)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:使用|启用|安装|列出).*(技能|skill)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:使用|调用|运行|列出).*(工具|tool)",
    r"^\s*(?:请|请帮我|帮我|麻烦你|麻烦帮我)?\s*(?:记住|记下来|保存到记忆|存到记忆|查记忆|查一下记忆|搜索记忆)",
];

/// A single decision: route + reason.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// `"chat"` or `"agent"`.
    pub route: String,
    /// Short human-readable reason.
    pub reason: String,
}

impl RouteDecision {
    /// True if this decision requires the full agent.
    pub fn requires_agent(&self) -> bool {
        self.route == "agent"
    }

    /// Builds a forced chat decision.
    pub fn forced_chat(reason: impl Into<String>) -> Self {
        Self {
            route: "chat".to_string(),
            reason: reason.into(),
        }
    }

    /// Builds a forced agent decision.
    pub fn forced_agent(reason: impl Into<String>) -> Self {
        Self {
            route: "agent".to_string(),
            reason: reason.into(),
        }
    }
}

/// Lightweight LLM used for ambiguous decisions. Mirrors the Python
/// `AgentCore.ask` signature.
#[async_trait::async_trait]
pub trait DeciderAgent: Send + Sync {
    async fn ask(
        &self,
        user_input: &str,
        history: Option<&[LLMMessage]>,
        extra_system_messages: Option<&[String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<DeciderAgentResponse, anyhow::Error>;
}

/// Response from a [`DeciderAgent`].
#[derive(Debug, Clone)]
pub struct DeciderAgentResponse {
    /// The plain-text content of the response.
    pub content: String,
    /// The finish reason (`"stop"`, `"length"`, ...).
    pub finish_reason: Option<String>,
}

/// Compiled regex patterns. The patterns are large and case-insensitive;
/// building them once avoids paying the cost on every turn.
static COMPILED_AGENT_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    AGENT_PATTERNS
        .iter()
        .filter_map(|p| {
            Regex::new(p)
                .map_err(|e| {
                    tracing::error!(pattern = %p, error = %e, "invalid agent pattern");
                    e
                })
                .ok()
        })
        .collect()
});

static ROUTE_FIELD_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\broute\b\s*[:=]\s*['"]?(chat|agent)['"]?"#)
        .expect("valid route field pattern")
});

static ROUTE_TOKEN_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)^\s*['"]?(chat|agent)['"]?\s*$"#)
        .expect("valid route token pattern")
});

/// The decision engine.
pub struct DecisionEngine {
    decider_agent: Option<Arc<dyn DeciderAgent>>,
    max_tokens: u32,
}

impl std::fmt::Debug for DecisionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionEngine")
            .field("max_tokens", &self.max_tokens)
            .field("has_decider", &self.decider_agent.is_some())
            .finish()
    }
}

impl DecisionEngine {
    /// Creates a new decision engine. If `decider_agent` is `None`, the
    /// engine only uses the regex rules.
    pub fn new(decider_agent: Option<Arc<dyn DeciderAgent>>, max_tokens: Option<u32>) -> Self {
        Self {
            decider_agent,
            max_tokens: max_tokens.unwrap_or(DEFAULT_DECISION_MAX_TOKENS).max(1),
        }
    }

    /// Decides how to route `user_input`. `route_mode` overrides the
    /// automatic decision when set to `ChatOnly` or `ForceAgent`.
    pub async fn decide(
        &self,
        user_input: &str,
        history: Option<&[LLMMessage]>,
        route_mode: RouteMode,
    ) -> RouteDecision {
        match route_mode {
            RouteMode::ChatOnly => return RouteDecision::forced_chat("Forced chat-only route"),
            RouteMode::ForceAgent => return RouteDecision::forced_agent("Forced full-agent route"),
            RouteMode::Auto => {}
        }

        if let Some(decision) = rule_based_decision(user_input) {
            return decision;
        }

        let Some(agent) = &self.decider_agent else {
            return RouteDecision::forced_chat("Fallback to lightweight chat");
        };

        let trimmed_history: Vec<LLMMessage> = history
            .map(|h| h.iter().rev().take(6).rev().cloned().collect())
            .unwrap_or_default();
        let extra = vec![DECISION_SYSTEM_PROMPT.to_string()];

        let response = match agent
            .ask(
                user_input,
                Some(&trimmed_history),
                Some(&extra),
                Some(0.0),
                Some(self.max_tokens),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "decider agent ask failed");
                return RouteDecision::forced_chat("Fallback to lightweight chat");
            }
        };
        let decision = parse_decision_response(&response.content);
        if response.finish_reason.as_deref() == Some("length") {
            warn!(
                route = %decision.route,
                "decision layer hit max_tokens limit and returned truncated output"
            );
        }
        decision
    }
}

/// Looks for a regex match in `user_input`.
pub fn rule_based_decision(user_input: &str) -> Option<RouteDecision> {
    let cleaned = user_input.trim();
    if cleaned.is_empty() {
        return Some(RouteDecision::forced_chat("Empty input"));
    }
    if matches_any_pattern(cleaned) {
        return Some(RouteDecision::forced_agent(
            "Likely workspace or tool task",
        ));
    }
    None
}

fn matches_any_pattern(text: &str) -> bool {
    COMPILED_AGENT_PATTERNS.iter().any(|r| r.is_match(text))
}

/// Parses the decider LLM's text response into a `RouteDecision`. The
/// parser first tries a JSON object, then falls back to a regex search
/// for `route: chat` / `route: agent` / a single bare token.
pub fn parse_decision_response(text: &str) -> RouteDecision {
    if let Some(parsed) = try_parse_json_object(text) {
        let route = parsed
            .get("route")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let reason = parsed
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("LLM decision")
            .to_string();
        if route == "chat" || route == "agent" {
            return RouteDecision { route, reason };
        }
    }
    if let Some(fallback) = extract_route_from_text(text) {
        return RouteDecision {
            route: fallback,
            reason: "LLM fallback parse".to_string(),
        };
    }
    RouteDecision::forced_chat("LLM fallback parse")
}

fn extract_route_from_text(text: &str) -> Option<String> {
    if let Some(caps) = ROUTE_FIELD_PATTERN.captures(text) {
        if let Some(m) = caps.get(1) {
            return Some(m.as_str().to_lowercase());
        }
    }
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if let Some(caps) = ROUTE_TOKEN_PATTERN.captures(stripped) {
            if let Some(m) = caps.get(1) {
                return Some(m.as_str().to_lowercase());
            }
        }
        break;
    }
    None
}

fn try_parse_json_object(text: &str) -> Option<Value> {
    let cleaned = text.trim();
    if cleaned.is_empty() {
        return None;
    }
    let mut candidates: Vec<&str> = vec![cleaned];
    if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if start < end {
            candidates.push(&cleaned[start..=end]);
        }
    }
    for candidate in candidates {
        if let Ok(Value::Object(_)) = serde_json::from_str::<Value>(candidate) {
            return serde_json::from_str(candidate).ok();
        }
    }
    None
}

/// Convenience helper for callers that already have a string mode and want
/// to call into the engine with a typed `RouteMode`.
pub fn parse_route_mode(value: &str) -> RouteMode {
    normalize_route_mode(Some(value))
}

/// Helper for converting a decision into a human-readable trace line.
pub fn describe_decision(decision: &RouteDecision) -> String {
    format!("route={} reason={}", decision.route, decision.reason)
}

#[allow(dead_code)]
fn _unused_message_content_to_text_silence(content: &echobot_core::models::MessageContent) -> String {
    message_content_to_text(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use echobot_core::models::{LLMMessage, MessageRole};

    #[test]
    fn empty_input_forces_chat() {
        let decision = rule_based_decision("   ").unwrap();
        assert_eq!(decision.route, "chat");
    }

    #[test]
    fn english_set_cron_matches_agent_pattern() {
        let decision = rule_based_decision("please set a cron to ping every minute").unwrap();
        assert_eq!(decision.route, "agent");
    }

    #[test]
    fn english_run_command_matches_agent_pattern() {
        let decision = rule_based_decision("run the test suite please").unwrap();
        assert_eq!(decision.route, "agent");
    }

    #[test]
    fn chinese_schedule_matches_agent_pattern() {
        // The pattern requires Arabic digits + Chinese unit (e.g. "10分钟后").
        let decision = rule_based_decision("10分钟后提醒我去喝水").unwrap();
        assert_eq!(decision.route, "agent");
        // The other Chinese schedule patterns cover "提醒我" without digits.
        let decision = rule_based_decision("设置提醒喝水").unwrap();
        assert_eq!(decision.route, "agent");
    }

    #[test]
    fn small_talk_does_not_match() {
        assert!(rule_based_decision("hi there!").is_none());
        assert!(rule_based_decision("今天天气真好").is_none());
    }

    #[test]
    fn parse_decision_response_accepts_clean_json() {
        let decision = parse_decision_response("{\"route\":\"agent\",\"reason\":\"needs a tool\"}");
        assert_eq!(decision.route, "agent");
        assert_eq!(decision.reason, "needs a tool");
    }

    #[test]
    fn parse_decision_response_accepts_fenced_json() {
        let decision = parse_decision_response("```json\n{\"route\":\"chat\",\"reason\":\"small talk\"}\n```");
        assert_eq!(decision.route, "chat");
    }

    #[test]
    fn parse_decision_response_falls_back_to_inline_route() {
        let decision = parse_decision_response("Some thinking... route: agent because we need files");
        assert_eq!(decision.route, "agent");
    }

    #[test]
    fn parse_decision_response_falls_back_to_bare_token() {
        let decision = parse_decision_response("agent\n");
        assert_eq!(decision.route, "agent");
    }

    #[test]
    fn parse_decision_response_defaults_to_chat() {
        let decision = parse_decision_response("I cannot decide, sorry.");
        assert_eq!(decision.route, "chat");
    }

    #[tokio::test]
    async fn route_mode_overrides_rule_decision() {
        let engine = DecisionEngine::new(None, None);
        let decision = engine
            .decide("please set a cron", None, RouteMode::ChatOnly)
            .await;
        assert_eq!(decision.route, "chat");
        let decision = engine
            .decide("hi there", None, RouteMode::ForceAgent)
            .await;
        assert_eq!(decision.route, "agent");
    }

    #[tokio::test]
    async fn engine_without_decider_falls_back_to_chat() {
        let engine = DecisionEngine::new(None, None);
        let decision = engine.decide("just a thought", None, RouteMode::Auto).await;
        assert_eq!(decision.route, "chat");
    }

    struct StubDecider;

    #[async_trait::async_trait]
    impl DeciderAgent for StubDecider {
        async fn ask(
            &self,
            _user_input: &str,
            _history: Option<&[LLMMessage]>,
            _extra: Option<&[String]>,
            _temperature: Option<f32>,
            _max_tokens: Option<u32>,
        ) -> Result<DeciderAgentResponse, anyhow::Error> {
            Ok(DeciderAgentResponse {
                content: "{\"route\":\"agent\",\"reason\":\"stub says agent\"}".to_string(),
                finish_reason: Some("stop".to_string()),
            })
        }
    }

    #[tokio::test]
    async fn engine_with_decider_returns_decider_choice() {
        let agent: Arc<dyn DeciderAgent> = Arc::new(StubDecider);
        let engine = DecisionEngine::new(Some(agent), None);
        let decision = engine.decide("totally ambiguous", None, RouteMode::Auto).await;
        assert_eq!(decision.route, "agent");
        assert_eq!(decision.reason, "stub says agent");
        // silence unused-import warning for MessageRole
        let _ = MessageRole::System;
    }
}
