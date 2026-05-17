use super::super::tool_calls::ToolDescriptor;
use super::dispatch::{decode_choice, role_str, tail, trim_trailing_slash};
use super::request::{float_env, truthy_env, uint_env};
use super::wire::{ChatRequestBody, ChatTemplateKwargs, Choice, ResponseMessage};
use crate::session::agent::adaptation::{
    GEMMA4_MODEL_FAMILY, QWEN3_6_MODEL_FAMILY, prepare_messages_for_openai_compat,
};
use crate::session::protocol::{LlmMessage, LlmRole};

fn empty_body(model: &str, max_tokens: u32) -> ChatRequestBody<'_> {
    ChatRequestBody {
        model,
        messages: vec![],
        stream: false,
        stream_options: None,
        max_tokens,
        seed: None,
        chat_template_kwargs: None,
        temperature: None,
        top_p: None,
        top_k: None,
        min_p: None,
        presence_penalty: None,
        repetition_penalty: None,
        tools: None,
        tool_choice: None,
    }
}

#[test]
fn request_body_omits_seed_and_kwargs_by_default() {
    // Default request: no seed, no chat_template_kwargs. Some
    // openai-compat proxies reject unknown keys, so the body
    // must stay minimal when the caller hasn't asked for the
    // new knobs.
    let body = empty_body("qwen3.6", 65_536);
    let json = serde_json::to_string(&body).unwrap();
    assert!(!json.contains("\"seed\""), "json: {json}");
    assert!(!json.contains("\"chat_template_kwargs\""), "json: {json}");
    assert!(!json.contains("\"temperature\""), "json: {json}");
    assert!(!json.contains("\"presence_penalty\""), "json: {json}");
}

#[test]
fn request_body_includes_seed_when_set() {
    let mut body = empty_body("qwen3.6", 64);
    body.seed = Some(42);
    let json = serde_json::to_string(&body).unwrap();
    assert!(json.contains("\"seed\":42"), "json: {json}");
}

#[test]
fn request_body_includes_enable_thinking_false_when_kwargs_set() {
    // The kwarg shape is what vLLM threads into the Qwen
    // chat template. Pin the exact wire shape so a refactor
    // doesn't silently rename / move the field.
    let mut body = empty_body("qwen3.6", 64);
    body.chat_template_kwargs = Some(ChatTemplateKwargs {
        enable_thinking: Some(false),
        thinking_budget: None,
    });
    let json = serde_json::to_string(&body).unwrap();
    assert!(
        json.contains("\"chat_template_kwargs\":{\"enable_thinking\":false}"),
        "json: {json}",
    );
}

#[test]
fn request_body_includes_thinking_budget_when_set() {
    let mut body = empty_body("qwen3.6", 64);
    body.chat_template_kwargs = Some(ChatTemplateKwargs {
        enable_thinking: None,
        thinking_budget: Some(2048),
    });
    let json = serde_json::to_string(&body).unwrap();
    assert!(
        json.contains("\"chat_template_kwargs\":{\"thinking_budget\":2048}"),
        "json: {json}",
    );
}

#[test]
fn request_body_serializes_sampling_knobs_when_set() {
    // All six sampling knobs are independent skip-if-None
    // fields. Pin their on-the-wire shape so a future refactor
    // can't silently rename them and break vLLM acceptance.
    let mut body = empty_body("qwen3.6", 64);
    body.temperature = Some(0.7);
    body.top_p = Some(0.8);
    body.top_k = Some(20);
    body.min_p = Some(0.0);
    body.presence_penalty = Some(1.5);
    body.repetition_penalty = Some(1.0);
    let json = serde_json::to_string(&body).unwrap();
    assert!(json.contains("\"temperature\":0.7"), "json: {json}");
    assert!(json.contains("\"top_p\":0.8"), "json: {json}");
    assert!(json.contains("\"top_k\":20"), "json: {json}");
    assert!(json.contains("\"min_p\":0.0"), "json: {json}");
    assert!(json.contains("\"presence_penalty\":1.5"), "json: {json}");
    assert!(json.contains("\"repetition_penalty\":1.0"), "json: {json}");
}

#[test]
fn request_body_serializes_tools_and_tool_choice_when_set() {
    let mut body = empty_body("qwen3.6", 64);
    body.tools = Some(vec![ToolDescriptor::function(
        "list_dir".into(),
        "List a directory".into(),
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
    )]);
    body.tool_choice = Some("auto");
    let v: serde_json::Value = serde_json::to_value(&body).unwrap();
    assert_eq!(v["tools"][0]["type"], "function");
    assert_eq!(v["tools"][0]["function"]["name"], "list_dir");
    assert_eq!(v["tool_choice"], "auto");
}

#[test]
fn request_body_omits_tools_by_default() {
    // Fence-mode callers (the default today) leave tools unset
    // so vLLM / LM Studio's tool-call parser stays dormant and
    // the wire shape is identical to the pre-Phase-B wire body.
    let body = empty_body("qwen3.6", 64);
    let json = serde_json::to_string(&body).unwrap();
    assert!(!json.contains("\"tools\""), "json: {json}");
    assert!(!json.contains("\"tool_choice\""), "json: {json}");
}

#[test]
fn role_str_maps_each_role() {
    assert_eq!(role_str(LlmRole::System), "system");
    assert_eq!(role_str(LlmRole::User), "user");
    assert_eq!(role_str(LlmRole::Assistant), "assistant");
}

#[test]
fn tail_passes_short_strings_through() {
    assert_eq!(tail("hello", 100), "hello");
}

#[test]
fn tail_walks_forward_to_char_boundary_on_multibyte_cut() {
    // Build a string where the cut at `s.len() - max` lands inside
    // a multibyte char. The helper walks FORWARD to the next char
    // boundary so we never return an invalid slice.
    let mut s = String::new();
    s.push_str(&"a".repeat(10));
    s.push('\u{2026}'); // 3 bytes
    s.push_str(&"b".repeat(10));
    // s.len() = 10 + 3 + 10 = 23. Pick max so the naive cut lands
    // mid-char: 23 - max = 11 (byte 1 of the 3-byte char).
    let out = tail(&s, 12);
    // The walk-forward lands at byte 13 (end of the multibyte char),
    // dropping the partial codepoint AND keeping 10 trailing 'b's.
    assert_eq!(out, "bbbbbbbbbb");
}

#[test]
fn truthy_env_reads_canonical_truthy_values() {
    let prior = std::env::var("SIM_FLOW_TEST_TRUTHY_VAR").ok();
    for v in ["1", "true", "True", "TRUE", "yes", "YES"] {
        // SAFETY: tests serialize env access already.
        unsafe {
            std::env::set_var("SIM_FLOW_TEST_TRUTHY_VAR", v);
        }
        assert!(truthy_env("SIM_FLOW_TEST_TRUTHY_VAR"), "{v}");
    }
    for v in ["0", "false", "no", "off", "FALSE", ""] {
        unsafe {
            std::env::set_var("SIM_FLOW_TEST_TRUTHY_VAR", v);
        }
        assert!(!truthy_env("SIM_FLOW_TEST_TRUTHY_VAR"), "{v}");
    }
    unsafe {
        std::env::remove_var("SIM_FLOW_TEST_TRUTHY_VAR");
    }
    assert!(!truthy_env("SIM_FLOW_TEST_TRUTHY_VAR"));
    // Restore.
    unsafe {
        match prior {
            Some(v) => std::env::set_var("SIM_FLOW_TEST_TRUTHY_VAR", v),
            None => std::env::remove_var("SIM_FLOW_TEST_TRUTHY_VAR"),
        }
    }
}

#[test]
fn float_env_and_uint_env_parse_or_return_none() {
    let prior = std::env::var("SIM_FLOW_TEST_NUM_VAR").ok();
    unsafe {
        std::env::set_var("SIM_FLOW_TEST_NUM_VAR", "1.5");
    }
    assert_eq!(float_env("SIM_FLOW_TEST_NUM_VAR"), Some(1.5));
    // Float string isn't a valid uint.
    assert_eq!(uint_env("SIM_FLOW_TEST_NUM_VAR"), None);
    unsafe {
        std::env::set_var("SIM_FLOW_TEST_NUM_VAR", "42");
    }
    assert_eq!(uint_env("SIM_FLOW_TEST_NUM_VAR"), Some(42));
    // Garbage -> None for both.
    unsafe {
        std::env::set_var("SIM_FLOW_TEST_NUM_VAR", "not a number");
    }
    assert!(float_env("SIM_FLOW_TEST_NUM_VAR").is_none());
    assert!(uint_env("SIM_FLOW_TEST_NUM_VAR").is_none());
    // Unset -> None.
    unsafe {
        std::env::remove_var("SIM_FLOW_TEST_NUM_VAR");
    }
    assert!(float_env("SIM_FLOW_TEST_NUM_VAR").is_none());
    assert!(uint_env("SIM_FLOW_TEST_NUM_VAR").is_none());
    // Restore.
    unsafe {
        match prior {
            Some(v) => std::env::set_var("SIM_FLOW_TEST_NUM_VAR", v),
            None => std::env::remove_var("SIM_FLOW_TEST_NUM_VAR"),
        }
    }
}

#[test]
fn trim_trailing_slash_keeps_paths_intact() {
    assert_eq!(trim_trailing_slash("http://x/v1"), "http://x/v1");
    assert_eq!(trim_trailing_slash("http://x/v1/"), "http://x/v1");
    // Repeated slashes are all stripped.
    assert_eq!(trim_trailing_slash("http://x/v1///"), "http://x/v1");
    assert_eq!(trim_trailing_slash(""), "");
}

fn msg(role: LlmRole, content: &str) -> LlmMessage {
    LlmMessage {
        role,
        content: content.into(),
        attachments: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

#[test]
fn merge_leading_system_collapses_multiple_to_one() {
    // vLLM / qwen chat templates require a single leading system
    // message; the orchestrator emits 4-5. Verify we collapse
    // them with the visible separator preserved.
    let messages = vec![
        msg(LlmRole::System, "first"),
        msg(LlmRole::System, "second"),
        msg(LlmRole::System, "third"),
        msg(LlmRole::User, "hi"),
    ];
    let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
    assert_eq!(merged.len(), 2);
    assert_eq!(role_str(merged[0].role), "system");
    assert!(merged[0].content.contains("first"));
    assert!(merged[0].content.contains("second"));
    assert!(merged[0].content.contains("third"));
    assert!(merged[0].content.contains("---"));
    assert_eq!(role_str(merged[1].role), "user");
    assert_eq!(merged[1].content.as_str(), "hi");
}

#[test]
fn merge_leading_system_passes_through_single_system() {
    let messages = vec![msg(LlmRole::System, "only"), msg(LlmRole::User, "hi")];
    let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].content.as_str(), "only");
}

#[test]
fn merge_leading_system_handles_no_system() {
    let messages = vec![msg(LlmRole::User, "hi")];
    let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
    assert_eq!(merged.len(), 1);
    assert_eq!(role_str(merged[0].role), "user");
}

#[test]
fn merge_leading_system_does_not_touch_mid_conversation_system() {
    // If the orchestrator ever emits a system message after a
    // user / assistant turn, leave it alone — the merge fix only
    // addresses the LEADING-stack case. A mid-conversation system
    // message will still get rejected by qwen / vLLM, but the
    // 4xx body will surface in the LlmError diagnostic so we'll
    // catch it instead of silently breaking the stack.
    let messages = vec![
        msg(LlmRole::System, "leading-1"),
        msg(LlmRole::System, "leading-2"),
        msg(LlmRole::User, "first"),
        msg(LlmRole::Assistant, "reply"),
        msg(LlmRole::System, "mid-stream"),
        msg(LlmRole::User, "second"),
    ];
    let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
    assert_eq!(merged.len(), 5); // 2 leading collapsed -> 1, plus 4 originals
    assert_eq!(role_str(merged[0].role), "system");
    assert!(merged[0].content.contains("leading-1"));
    assert!(merged[0].content.contains("leading-2"));
    assert_eq!(role_str(merged[1].role), "user");
    assert_eq!(role_str(merged[2].role), "assistant");
    assert_eq!(role_str(merged[3].role), "system"); // mid-stream system left in place
    assert_eq!(merged[3].content.as_str(), "mid-stream");
    assert_eq!(role_str(merged[4].role), "user");
}

fn choice(content: Option<&str>, reasoning: Option<&str>, finish: Option<&str>) -> Choice {
    Choice {
        message: ResponseMessage {
            content: content.map(String::from),
            reasoning: reasoning.map(String::from),
            tool_calls: None,
        },
        finish_reason: finish.map(String::from),
    }
}

#[test]
fn decode_choice_returns_content_on_normal_stop() {
    let c = choice(Some("hello"), None, Some("stop"));
    assert_eq!(
        decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap(),
        "hello"
    );
}

#[test]
fn decode_choice_falls_back_to_reasoning_when_content_empty() {
    let c = choice(None, Some("thinking text"), Some("stop"));
    assert_eq!(
        decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap(),
        "thinking text"
    );
}

#[test]
fn response_message_deserializes_reasoning_content_alias() {
    // vLLM with `--reasoning-parser qwen3` emits the thinking
    // text in a field called `reasoning_content`, not
    // `reasoning`. Without the serde alias, serde drops the
    // field silently and we lose the thinking output entirely.
    let body = r#"{"content":null,"reasoning_content":"step 1: ..."}"#;
    let msg: ResponseMessage = serde_json::from_str(body).unwrap();
    assert_eq!(msg.content, None);
    assert_eq!(msg.reasoning.as_deref(), Some("step 1: ..."));
}

#[test]
fn decode_choice_strips_qwen_think_tags_from_content() {
    let c = choice(Some("<think>plan</think>final answer"), None, Some("stop"));
    assert_eq!(
        decode_choice(Some(c), &QWEN3_6_MODEL_FAMILY).unwrap(),
        "final answer"
    );
}

#[test]
fn decode_choice_errors_on_finish_length_with_content() {
    // The today's-bug case: vLLM returns a partially-written
    // markdown file with finish_reason=length. We must NOT
    // commit the partial bytes; the orchestrator's LlmError
    // path engages instead.
    let c = choice(
        Some("# Coverage\n\n```bash\ncargo llvm-cov"),
        None,
        Some("length"),
    );
    let err = decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("truncated at max_tokens"), "got: {msg}");
    assert!(msg.contains("cargo llvm-cov"), "tail should appear: {msg}");
}

#[test]
fn decode_choice_errors_on_finish_length_with_only_reasoning() {
    let c = choice(None, Some("step 1: ..."), Some("length"));
    let err = decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap_err();
    assert!(format!("{err}").contains("truncated at max_tokens"));
}

#[test]
fn decode_choice_returns_empty_when_no_choice() {
    assert_eq!(decode_choice(None, &GEMMA4_MODEL_FAMILY).unwrap(), "");
}
