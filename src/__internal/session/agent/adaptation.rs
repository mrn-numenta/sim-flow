use crate::session::protocol::{LlmMessage, LlmRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProfileId {
    OpenAiCompatGeneric,
    ClaudeCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThoughtMarkerStyle {
    None,
    QwenThinkTag,
    KimiThinkTag,
    GemmaThinkTag,
    AnthropicThinkingBlocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningHistoryPolicy {
    PreserveAll,
    DropPriorReasoning,
    RuntimeControlled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCapabilityProfile {
    pub id: RuntimeProfileId,
    pub request_format: &'static str,
    pub credential_policy: &'static str,
    pub system_prompt_mode: &'static str,
    pub collapse_leading_system_messages: bool,
    pub supports_structured_reasoning: bool,
    pub supports_structured_tool_calls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFamilyProfile {
    pub id: &'static str,
    pub thought_marker_style: ThoughtMarkerStyle,
    pub prefers_media_before_text: bool,
    pub supports_thinking_controls: bool,
    pub thinking_control_mode: &'static str,
    pub reasoning_history_policy: ReasoningHistoryPolicy,
}

pub const OPENAI_COMPAT_GENERIC_RUNTIME: RuntimeCapabilityProfile = RuntimeCapabilityProfile {
    id: RuntimeProfileId::OpenAiCompatGeneric,
    request_format: "openai_chat_completions",
    credential_policy: "shared_provider_chain",
    system_prompt_mode: "collapsed-leading-message",
    collapse_leading_system_messages: true,
    supports_structured_reasoning: true,
    supports_structured_tool_calls: true,
};

pub const CLAUDE_CLI_RUNTIME: RuntimeCapabilityProfile = RuntimeCapabilityProfile {
    id: RuntimeProfileId::ClaudeCli,
    request_format: "subprocess_prompt",
    credential_policy: "host_managed",
    system_prompt_mode: "message-array",
    collapse_leading_system_messages: false,
    supports_structured_reasoning: false,
    supports_structured_tool_calls: false,
};

pub const GENERIC_CHAT_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "generic_chat",
    thought_marker_style: ThoughtMarkerStyle::None,
    prefers_media_before_text: false,
    supports_thinking_controls: false,
    thinking_control_mode: "none",
    reasoning_history_policy: ReasoningHistoryPolicy::PreserveAll,
};

pub const GEMMA4_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "gemma4",
    thought_marker_style: ThoughtMarkerStyle::GemmaThinkTag,
    prefers_media_before_text: true,
    supports_thinking_controls: true,
    thinking_control_mode: "prompt_tag",
    reasoning_history_policy: ReasoningHistoryPolicy::DropPriorReasoning,
};

pub const QWEN3_6_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "qwen3_6",
    thought_marker_style: ThoughtMarkerStyle::QwenThinkTag,
    prefers_media_before_text: false,
    supports_thinking_controls: true,
    thinking_control_mode: "runtime_flag",
    reasoning_history_policy: ReasoningHistoryPolicy::RuntimeControlled,
};

pub const KIMI_VL_THINKING_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "kimi_vl_thinking",
    thought_marker_style: ThoughtMarkerStyle::KimiThinkTag,
    prefers_media_before_text: true,
    supports_thinking_controls: false,
    thinking_control_mode: "none",
    reasoning_history_policy: ReasoningHistoryPolicy::PreserveAll,
};

pub const CLAUDE_MESSAGES_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "claude_messages",
    thought_marker_style: ThoughtMarkerStyle::AnthropicThinkingBlocks,
    prefers_media_before_text: false,
    supports_thinking_controls: true,
    thinking_control_mode: "runtime_flag",
    reasoning_history_policy: ReasoningHistoryPolicy::DropPriorReasoning,
};

pub fn model_family_by_id(id: Option<&str>) -> Option<&'static ModelFamilyProfile> {
    match id {
        Some("generic_chat") => Some(&GENERIC_CHAT_MODEL_FAMILY),
        Some("gemma4") => Some(&GEMMA4_MODEL_FAMILY),
        Some("qwen3_6") => Some(&QWEN3_6_MODEL_FAMILY),
        Some("kimi_vl_thinking") => Some(&KIMI_VL_THINKING_MODEL_FAMILY),
        Some("claude_messages") => Some(&CLAUDE_MESSAGES_MODEL_FAMILY),
        _ => None,
    }
}

pub fn infer_model_family(model: Option<&str>) -> &'static ModelFamilyProfile {
    let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) else {
        return &GENERIC_CHAT_MODEL_FAMILY;
    };
    let lowered = model.to_ascii_lowercase();
    if lowered.contains("gemma-4") {
        &GEMMA4_MODEL_FAMILY
    } else if lowered.contains("qwen3.6")
        || lowered.contains("qwen-3.6")
        || lowered.contains("qwen3-6")
    {
        &QWEN3_6_MODEL_FAMILY
    } else if lowered.contains("kimi-vl") {
        &KIMI_VL_THINKING_MODEL_FAMILY
    } else if lowered.contains("claude") {
        &CLAUDE_MESSAGES_MODEL_FAMILY
    } else {
        &GENERIC_CHAT_MODEL_FAMILY
    }
}

pub fn resolve_model_family(
    explicit_id: Option<&str>,
    model: Option<&str>,
) -> &'static ModelFamilyProfile {
    model_family_by_id(explicit_id).unwrap_or_else(|| infer_model_family(model))
}

pub fn apply_reasoning_history_policy(
    messages: &[LlmMessage],
    family: &ModelFamilyProfile,
) -> Vec<LlmMessage> {
    if family.reasoning_history_policy != ReasoningHistoryPolicy::DropPriorReasoning {
        return messages.to_vec();
    }
    messages
        .iter()
        .map(|message| LlmMessage {
            role: message.role,
            content: strip_known_reasoning_markers(&message.content, family)
                .trim()
                .to_string(),
            attachments: message.attachments.clone(),
        })
        .collect()
}

pub fn normalize_response_text(family: &ModelFamilyProfile, text: &str) -> String {
    strip_known_reasoning_markers(text, family)
        .trim()
        .to_string()
}

pub fn merge_leading_system_messages(messages: &[LlmMessage], separator: &str) -> Vec<LlmMessage> {
    let leading_system_count = messages
        .iter()
        .take_while(|m| matches!(m.role, LlmRole::System))
        .count();
    if leading_system_count <= 1 {
        return messages.to_vec();
    }
    let mut out = Vec::with_capacity(messages.len() - leading_system_count + 1);
    let mut merged_content = String::new();
    for (idx, message) in messages.iter().take(leading_system_count).enumerate() {
        if idx > 0 {
            merged_content.push_str(separator);
        }
        merged_content.push_str(&message.content);
    }
    let mut merged_attachments = Vec::new();
    for message in messages.iter().take(leading_system_count) {
        merged_attachments.extend(message.attachments.iter().cloned());
    }
    out.push(LlmMessage {
        role: LlmRole::System,
        content: merged_content,
        attachments: merged_attachments,
    });
    out.extend(messages.iter().skip(leading_system_count).cloned());
    out
}

pub fn prepare_messages_for_openai_compat(
    messages: &[LlmMessage],
    family: &ModelFamilyProfile,
) -> Vec<LlmMessage> {
    let messages = apply_reasoning_history_policy(messages, family);
    merge_leading_system_messages(&messages, "\n\n---\n\n")
}

fn strip_known_reasoning_markers(content: &str, family: &ModelFamilyProfile) -> String {
    match family.thought_marker_style {
        ThoughtMarkerStyle::QwenThinkTag => {
            strip_delimited_sections(content, "<think>", "</think>")
        }
        ThoughtMarkerStyle::KimiThinkTag => {
            strip_delimited_sections(content, "◁think▷", "◁/think▷")
        }
        _ => content.to_string(),
    }
}

fn strip_delimited_sections(content: &str, start: &str, end: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    while let Some(start_rel) = content[cursor..].find(start) {
        let start_idx = cursor + start_rel;
        out.push_str(&content[cursor..start_idx]);
        let after_start = start_idx + start.len();
        if let Some(end_rel) = content[after_start..].find(end) {
            cursor = after_start + end_rel + end.len();
        } else {
            cursor = content.len();
            break;
        }
    }
    out.push_str(&content[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{LlmAttachment, LlmRole};

    fn msg(role: LlmRole, content: &str) -> LlmMessage {
        LlmMessage {
            role,
            content: content.into(),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn infer_model_family_matches_known_ids() {
        assert_eq!(
            infer_model_family(Some("google/gemma-4-27b-it")).id,
            "gemma4"
        );
        assert_eq!(
            infer_model_family(Some("Qwen/Qwen3.6-35B-A3B")).id,
            "qwen3_6"
        );
        assert_eq!(
            infer_model_family(Some("moonshotai/Kimi-VL-A3B-Thinking-2506")).id,
            "kimi_vl_thinking"
        );
        assert_eq!(
            infer_model_family(Some("claude-sonnet-4-6")).id,
            "claude_messages"
        );
        assert_eq!(infer_model_family(Some("gpt-4o-mini")).id, "generic_chat");
    }

    #[test]
    fn resolve_model_family_honors_explicit_override() {
        assert_eq!(
            resolve_model_family(Some("gemma4"), Some("moonshotai/Kimi-VL-A3B-Thinking-2506")).id,
            "gemma4"
        );
    }

    #[test]
    fn normalize_response_text_strips_qwen_think_tags() {
        assert_eq!(
            normalize_response_text(&QWEN3_6_MODEL_FAMILY, "<think>plan</think>answer"),
            "answer"
        );
    }

    #[test]
    fn normalize_response_text_strips_kimi_think_tags() {
        assert_eq!(
            normalize_response_text(&KIMI_VL_THINKING_MODEL_FAMILY, "◁think▷plan◁/think▷answer"),
            "answer"
        );
    }

    #[test]
    fn merge_leading_system_messages_collapses_and_keeps_attachments() {
        let a = LlmAttachment {
            mime: "image/png".into(),
            data: "AAA".into(),
            source: Some("a.png".into()),
        };
        let b = LlmAttachment {
            mime: "image/png".into(),
            data: "BBB".into(),
            source: Some("b.png".into()),
        };
        let messages = vec![
            LlmMessage {
                role: LlmRole::System,
                content: "first".into(),
                attachments: vec![a.clone()],
            },
            LlmMessage {
                role: LlmRole::System,
                content: "second".into(),
                attachments: vec![b.clone()],
            },
            msg(LlmRole::User, "hi"),
        ];
        let merged = merge_leading_system_messages(&messages, "\n\n");
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].content, "first\n\nsecond");
        assert_eq!(merged[0].attachments.len(), 2);
        assert_eq!(merged[0].attachments[0].mime, a.mime);
        assert_eq!(merged[0].attachments[0].data, a.data);
        assert_eq!(merged[0].attachments[0].source, a.source);
        assert_eq!(merged[0].attachments[1].mime, b.mime);
        assert_eq!(merged[0].attachments[1].data, b.data);
        assert_eq!(merged[0].attachments[1].source, b.source);
    }

    #[test]
    fn prepare_messages_for_openai_compat_cleans_and_merges() {
        let messages = vec![
            msg(LlmRole::System, "rules"),
            msg(LlmRole::System, "tools"),
            msg(LlmRole::Assistant, "<think>plan</think>answer"),
        ];
        let prepared = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[0].role, LlmRole::System);
        assert!(prepared[0].content.contains("rules"));
        assert!(prepared[0].content.contains("tools"));
    }
}
