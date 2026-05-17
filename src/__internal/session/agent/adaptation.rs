use crate::session::protocol::{LlmMessage, LlmRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProfileId {
    OpenAiCompatGeneric,
    ClaudeCli,
}

impl RuntimeProfileId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatGeneric => "openai_compat_generic",
            Self::ClaudeCli => "claude_cli",
        }
    }
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

/// Sampling parameters a model family expects in NON-THINKING mode.
///
/// Used when the orchestrator runs the family with thinking disabled
/// (`disable_thinking == true`). The values get serialized into the
/// chat-completions request body, overriding any server-side defaults
/// the operator has configured. Sourced from each model's official
/// guidance:
///
/// - **qwen3_6**: per the Qwen3.6-27B Hugging Face model card,
///   non-thinking (Instruct) mode wants `temperature=0.7, top_p=0.80,
///   top_k=20, min_p=0.0, presence_penalty=1.5, repetition_penalty=1.0`.
///   The card explicitly flags `presence_penalty=1.5` as the lever to
///   "reduce endless repetitions" -- maps directly onto our
///   `runaway-loop` and re-reading-without-writing patterns in the
///   model-robustness study.
///
/// `None` on a family means "no client-side override; let the server's
/// configured defaults stand." That's the right default for
/// `generic_chat` (we don't know the model's pedigree) and
/// `claude_messages` (the Anthropic API has its own defaults that work
/// well; tuning per-vendor is the operator's job, not ours).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplingDefaults {
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub min_p: f32,
    pub presence_penalty: f32,
    pub repetition_penalty: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelFamilyProfile {
    pub id: &'static str,
    pub thought_marker_style: ThoughtMarkerStyle,
    pub prefers_media_before_text: bool,
    pub supports_thinking_controls: bool,
    pub thinking_control_mode: &'static str,
    pub reasoning_history_policy: ReasoningHistoryPolicy,
    /// True when this family routinely emits the critique JSON
    /// as bare prose / a ```json fence rather than the canonical
    /// ```docs/critiques/<step>-critique.json fence the
    /// artifact-write convention asks for. The orchestrator's
    /// `salvage_critique_json` path catches both shapes; this
    /// flag downgrades the post-salvage diagnostic from
    /// `Warning` to `Info` so the dashboard's chat panel doesn't
    /// scare the operator with a yellow banner on a 100%-expected
    /// path. Confirmed for `qwen3_6` in Phase 0 of the
    /// model-robustness study (4/4 critique sessions across 3
    /// trials hit the salvage). Hypothesized true for similar
    /// verbose-tool-use families (`gemma4`, `kimi_vl_thinking`)
    /// pending Phase 1 confirmation; `claude_messages` is false
    /// (Claude reliably emits fenced blocks) and
    /// `generic_chat` is false (default = warn for unknown
    /// models).
    pub prefers_bare_json_critique: bool,
    /// Sampling defaults to send when the orchestrator runs this
    /// family in NON-THINKING mode. `None` means "no override;
    /// server defaults stand." See `SamplingDefaults` for sources.
    pub non_thinking_sampling: Option<SamplingDefaults>,
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
    prefers_bare_json_critique: false,
    non_thinking_sampling: None,
};

pub const GEMMA4_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "gemma4",
    thought_marker_style: ThoughtMarkerStyle::GemmaThinkTag,
    prefers_media_before_text: true,
    supports_thinking_controls: true,
    thinking_control_mode: "prompt_tag",
    reasoning_history_policy: ReasoningHistoryPolicy::DropPriorReasoning,
    // Hypothesized true (similar verbose-tool-use style to
    // Qwen3.6); Phase 1 of the model-robustness study will
    // confirm or reset.
    prefers_bare_json_critique: true,
    // No published non-thinking guidance for Gemma; let server
    // defaults stand. Revisit during Phase 1 LM Studio sweep.
    non_thinking_sampling: None,
};

/// Per the Qwen3.6-27B Hugging Face model card, non-thinking
/// (Instruct) mode. `presence_penalty=1.5` is the card's explicit
/// lever against endless repetition. Direct mitigation for the
/// `runaway-loop` anomaly and the re-reading-without-writing
/// pattern that drives `work-no-artifact`.
const QWEN3_6_NON_THINKING_SAMPLING: SamplingDefaults = SamplingDefaults {
    temperature: 0.7,
    top_p: 0.80,
    top_k: 20,
    min_p: 0.0,
    presence_penalty: 1.5,
    repetition_penalty: 1.0,
};

pub const QWEN3_6_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "qwen3_6",
    thought_marker_style: ThoughtMarkerStyle::QwenThinkTag,
    prefers_media_before_text: false,
    supports_thinking_controls: true,
    thinking_control_mode: "runtime_flag",
    reasoning_history_policy: ReasoningHistoryPolicy::RuntimeControlled,
    // Confirmed in Phase 0 of the model-robustness study:
    // 4/4 critique sessions across 3 trials hit
    // `salvage_critique_json`.
    prefers_bare_json_critique: true,
    non_thinking_sampling: Some(QWEN3_6_NON_THINKING_SAMPLING),
};

pub const KIMI_VL_THINKING_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "kimi_vl_thinking",
    thought_marker_style: ThoughtMarkerStyle::KimiThinkTag,
    prefers_media_before_text: true,
    supports_thinking_controls: false,
    thinking_control_mode: "none",
    reasoning_history_policy: ReasoningHistoryPolicy::PreserveAll,
    // Hypothesized true pending Phase 1 confirmation.
    prefers_bare_json_critique: true,
    // Kimi is a thinking-only family in our usage; non-thinking
    // sampling doesn't apply.
    non_thinking_sampling: None,
};

pub const CLAUDE_MESSAGES_MODEL_FAMILY: ModelFamilyProfile = ModelFamilyProfile {
    id: "claude_messages",
    thought_marker_style: ThoughtMarkerStyle::AnthropicThinkingBlocks,
    prefers_media_before_text: false,
    supports_thinking_controls: true,
    thinking_control_mode: "runtime_flag",
    reasoning_history_policy: ReasoningHistoryPolicy::DropPriorReasoning,
    // Claude reliably emits the fenced ```docs/critiques/...
    // block per the prompt; salvage on Claude IS a bug worth
    // surfacing.
    prefers_bare_json_critique: false,
    // Anthropic API has well-tuned defaults; client-side overrides
    // are the operator's job, not ours.
    non_thinking_sampling: None,
};

pub fn runtime_profile_by_id(id: Option<&str>) -> Option<RuntimeCapabilityProfile> {
    match id {
        Some("openai_compat_generic") => Some(OPENAI_COMPAT_GENERIC_RUNTIME),
        Some("claude_cli") => Some(CLAUDE_CLI_RUNTIME),
        _ => None,
    }
}

pub fn resolve_runtime_profile(
    explicit_id: Option<&str>,
    fallback: RuntimeCapabilityProfile,
    allowed_ids: &[&str],
) -> crate::Result<RuntimeCapabilityProfile> {
    let Some(explicit_id) = explicit_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(fallback);
    };
    let Some(profile) = runtime_profile_by_id(Some(explicit_id)) else {
        return Err(crate::Error::Client(format!(
            "unknown runtime capability profile `{explicit_id}`; known ids: openai_compat_generic, claude_cli"
        )));
    };
    if !allowed_ids.contains(&profile.id.as_str()) {
        return Err(crate::Error::Client(format!(
            "runtime capability profile `{explicit_id}` is not compatible here; allowed ids: {}",
            allowed_ids.join(", ")
        )));
    }
    Ok(profile)
}

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
            tool_call_id: message.tool_call_id.clone(),
            tool_calls: message.tool_calls.clone(),
            // DropPriorReasoning: don't replay this turn's reasoning
            // back to the model. Families with this policy
            // (gemma4, claude_messages) prefer a clean visible-answer-
            // only history; reasoning would either confuse the chat
            // template or double-render in the next turn's thinking.
            reasoning: None,
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
        tool_call_id: None,
        tool_calls: Vec::new(),
        reasoning: None,
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
        // qwen-code's TaggedThinkingParser accepts both <think> and
        // <thinking> as aliases (with their close tags) and matches
        // case-insensitively. qwen3.6 has been seen to emit <Think>
        // and <thinking> on quick turns, so the strip pass must
        // tolerate every shape upstream tolerates.
        ThoughtMarkerStyle::QwenThinkTag => strip_delimited_sections_ci(
            content,
            &[("<think>", "</think>"), ("<thinking>", "</thinking>")],
        ),
        ThoughtMarkerStyle::KimiThinkTag => {
            strip_delimited_sections_ci(content, &[("◁think▷", "◁/think▷")])
        }
        _ => content.to_string(),
    }
}

fn strip_delimited_sections_ci(content: &str, pairs: &[(&str, &str)]) -> String {
    // ASCII-case-fold the haystack so byte offsets stay aligned with
    // the original. Unicode `to_lowercase()` can change byte length
    // for some characters (e.g. ß -> "ss"), which would corrupt the
    // indices we splice with. Our patterns are all ASCII letters
    // wrapped in either `<...>` or the Kimi `◁...▷` glyphs, so
    // ASCII-folding is sufficient.
    let mut lower_bytes = content.as_bytes().to_vec();
    lower_bytes.make_ascii_lowercase();
    let lower = match std::str::from_utf8(&lower_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => content.to_string(),
    };

    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    while cursor < content.len() {
        let Some((start_idx, matched_start, matched_end)) =
            find_first_start_ci(&lower, cursor, pairs)
        else {
            break;
        };
        out.push_str(&content[cursor..start_idx]);
        let after_start = start_idx + matched_start.len();
        if let Some(end_rel) = lower[after_start..].find(matched_end) {
            cursor = after_start + end_rel + matched_end.len();
        } else {
            cursor = content.len();
            break;
        }
    }
    out.push_str(&content[cursor..]);
    out
}

fn find_first_start_ci<'a>(
    lower: &str,
    cursor: usize,
    pairs: &'a [(&'a str, &'a str)],
) -> Option<(usize, &'a str, &'a str)> {
    let mut best: Option<(usize, &str, &str)> = None;
    for (start, end) in pairs {
        if let Some(rel) = lower[cursor..].find(start) {
            let abs = cursor + rel;
            if best.map(|(b, _, _)| abs < b).unwrap_or(true) {
                best = Some((abs, start, end));
            }
        }
    }
    best
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
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
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
    fn normalize_response_text_strips_qwen_thinking_alias() {
        // qwen-code's TaggedThinkingParser accepts <thinking> as an
        // alias for <think>. qwen3.6 has been observed emitting both
        // shapes; the strip pass must catch each.
        assert_eq!(
            normalize_response_text(&QWEN3_6_MODEL_FAMILY, "<thinking>plan</thinking>answer"),
            "answer"
        );
    }

    #[test]
    fn normalize_response_text_strips_qwen_think_tags_case_insensitive() {
        // Case-insensitive matching mirrors qwen-code's parser: it
        // pre-lowers the buffer once per call so `<Think>` / `<THINK>`
        // are detected just like `<think>`.
        assert_eq!(
            normalize_response_text(&QWEN3_6_MODEL_FAMILY, "<Think>plan</Think>answer"),
            "answer"
        );
        assert_eq!(
            normalize_response_text(&QWEN3_6_MODEL_FAMILY, "<THINKING>plan</THINKING>answer"),
            "answer"
        );
    }

    #[test]
    fn normalize_response_text_handles_mixed_think_and_thinking() {
        // First a <think>, then a <thinking>; both should be stripped.
        assert_eq!(
            normalize_response_text(
                &QWEN3_6_MODEL_FAMILY,
                "a<think>x</think>b<thinking>y</thinking>c"
            ),
            "abc"
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
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning: None,
            },
            LlmMessage {
                role: LlmRole::System,
                content: "second".into(),
                attachments: vec![b.clone()],
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning: None,
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
    fn prefers_bare_json_critique_is_set_per_family() {
        // Verbose-tool-use families routinely emit critique JSON
        // as bare prose / ```json fences -- confirmed for qwen3_6
        // in Phase 0 of the model-robustness study (4/4 critique
        // sessions across 3 trials). The orchestrator downgrades
        // the post-salvage diagnostic to Info for these families
        // so the dashboard doesn't flag the expected path with a
        // yellow banner. Families that should reliably emit the
        // canonical fenced block (claude_messages, generic_chat)
        // still warn on salvage.
        //
        // Look up each family through `model_family_by_id` so
        // clippy doesn't const-fold the constants away into a
        // bare-boolean assertion.
        let qwen = model_family_by_id(Some("qwen3_6")).expect("qwen3_6 registered");
        let gemma = model_family_by_id(Some("gemma4")).expect("gemma4 registered");
        let kimi =
            model_family_by_id(Some("kimi_vl_thinking")).expect("kimi_vl_thinking registered");
        let claude =
            model_family_by_id(Some("claude_messages")).expect("claude_messages registered");
        let generic = model_family_by_id(Some("generic_chat")).expect("generic_chat registered");
        assert!(qwen.prefers_bare_json_critique);
        assert!(gemma.prefers_bare_json_critique);
        assert!(kimi.prefers_bare_json_critique);
        assert!(!claude.prefers_bare_json_critique);
        assert!(!generic.prefers_bare_json_critique);
    }

    #[test]
    fn qwen3_6_carries_non_thinking_sampling_per_hf_card() {
        // Per the Qwen3.6-27B Hugging Face model card,
        // non-thinking (Instruct) mode: temp=0.7, top_p=0.80,
        // top_k=20, min_p=0.0, presence_penalty=1.5,
        // repetition_penalty=1.0. presence_penalty=1.5 in
        // particular is the card's explicit lever against
        // endless repetition -- maps onto our runaway-loop and
        // re-reading-without-writing anomalies.
        let qwen = model_family_by_id(Some("qwen3_6")).expect("qwen3_6 registered");
        let s = qwen
            .non_thinking_sampling
            .expect("qwen3_6 carries non-thinking sampling defaults");
        assert!((s.temperature - 0.7).abs() < 1e-6);
        assert!((s.top_p - 0.80).abs() < 1e-6);
        assert_eq!(s.top_k, 20);
        assert!((s.min_p - 0.0).abs() < 1e-6);
        assert!((s.presence_penalty - 1.5).abs() < 1e-6);
        assert!((s.repetition_penalty - 1.0).abs() < 1e-6);
    }

    #[test]
    fn non_thinking_sampling_is_unset_on_unspecified_families() {
        // Families without explicit guidance (or whose tuning is
        // the operator's job, like Claude) carry None so the
        // server's defaults stand and we don't surprise users.
        for id in [
            "generic_chat",
            "gemma4",
            "kimi_vl_thinking",
            "claude_messages",
        ] {
            let f = model_family_by_id(Some(id)).expect("family registered");
            assert!(
                f.non_thinking_sampling.is_none(),
                "{id} unexpectedly carries non-thinking sampling defaults"
            );
        }
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

    #[test]
    fn model_family_by_id_maps_each_known_family() {
        assert!(model_family_by_id(Some("generic_chat")).is_some());
        assert!(model_family_by_id(Some("gemma4")).is_some());
        assert!(model_family_by_id(Some("qwen3_6")).is_some());
        assert!(model_family_by_id(Some("kimi_vl_thinking")).is_some());
        assert!(model_family_by_id(Some("claude_messages")).is_some());
        // Unknown id and None both return None.
        assert!(model_family_by_id(Some("not-a-family")).is_none());
        assert!(model_family_by_id(None).is_none());
    }

    #[test]
    fn infer_model_family_picks_specialized_profile_for_known_model_names() {
        // Gemma-4 substring.
        assert_eq!(
            infer_model_family(Some("google/gemma-4-12b-it")).id,
            GEMMA4_MODEL_FAMILY.id,
        );
        // qwen3.6 / qwen-3.6 / qwen3-6 all match.
        for m in ["qwen3.6", "Qwen-3.6-72B", "qwen3-6-coder"] {
            assert_eq!(
                infer_model_family(Some(m)).id,
                QWEN3_6_MODEL_FAMILY.id,
                "{m}"
            );
        }
        // kimi-vl substring.
        assert_eq!(
            infer_model_family(Some("kimi-vl-thinking")).id,
            KIMI_VL_THINKING_MODEL_FAMILY.id,
        );
        // claude substring.
        assert_eq!(
            infer_model_family(Some("claude-opus-4-7")).id,
            CLAUDE_MESSAGES_MODEL_FAMILY.id,
        );
        // Unknown model -> generic_chat.
        assert_eq!(
            infer_model_family(Some("mistral-large")).id,
            GENERIC_CHAT_MODEL_FAMILY.id,
        );
        // Empty / whitespace / None all fall to generic.
        for m in [Some(""), Some("   "), None] {
            assert_eq!(infer_model_family(m).id, GENERIC_CHAT_MODEL_FAMILY.id);
        }
    }

    #[test]
    fn resolve_model_family_prefers_explicit_id_over_model_inference() {
        // Explicit overrides the model-name inference.
        let fam = resolve_model_family(Some("claude_messages"), Some("gemma-4-12b"));
        assert_eq!(fam.id, CLAUDE_MESSAGES_MODEL_FAMILY.id);
        // No explicit -> infer from model.
        let fam2 = resolve_model_family(None, Some("gemma-4-12b"));
        assert_eq!(fam2.id, GEMMA4_MODEL_FAMILY.id);
        // Unknown explicit -> falls back to inference.
        let fam3 = resolve_model_family(Some("not-a-family"), Some("claude-opus"));
        assert_eq!(fam3.id, CLAUDE_MESSAGES_MODEL_FAMILY.id);
    }

    #[test]
    fn resolve_runtime_profile_uses_fallback_when_explicit_is_blank() {
        // None / empty / whitespace -> fallback.
        for id in [None, Some(""), Some("   ")] {
            let r = resolve_runtime_profile(
                id,
                OPENAI_COMPAT_GENERIC_RUNTIME,
                &[OPENAI_COMPAT_GENERIC_RUNTIME.id.as_str()],
            )
            .unwrap();
            assert_eq!(r.id, OPENAI_COMPAT_GENERIC_RUNTIME.id);
        }
    }

    #[test]
    fn resolve_runtime_profile_errors_on_unknown_or_incompatible_id() {
        // Unknown id -> Client error.
        let r = resolve_runtime_profile(
            Some("not-a-real-profile"),
            OPENAI_COMPAT_GENERIC_RUNTIME,
            &[OPENAI_COMPAT_GENERIC_RUNTIME.id.as_str()],
        );
        assert!(r.is_err());
        // Known but not in allowed_ids -> Client error.
        let r = resolve_runtime_profile(
            Some(CLAUDE_CLI_RUNTIME.id.as_str()),
            OPENAI_COMPAT_GENERIC_RUNTIME,
            &[OPENAI_COMPAT_GENERIC_RUNTIME.id.as_str()],
        );
        assert!(r.is_err());
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("not compatible"));
    }
}
