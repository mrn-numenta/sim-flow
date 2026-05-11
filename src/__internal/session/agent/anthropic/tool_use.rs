//! Native Anthropic tool-use wire shape.
//!
//! Anthropic's Messages API expresses tool calls and replies as
//! content blocks inside a per-message `content: [...]` array,
//! unlike OpenAI which puts them in a sibling `tool_calls: [...]`
//! field. The block types we care about:
//!
//! - Request side (assistant outgoing): `{type: "text", text}` and
//!   `{type: "tool_use", id, name, input}`.
//! - Request side (user incoming): `{type: "text", text}` and
//!   `{type: "tool_result", tool_use_id, content}`.
//! - Response side (assistant): same union as outgoing assistant.
//!
//! Tools are advertised via a top-level `tools: [{name, description,
//! input_schema}]` field on the request body. Note `input_schema`
//! (not `parameters`) and no `function` wrapper -- the differences
//! from OpenAI's `{type: "function", function: {...}}` shape are
//! exactly why the per-backend converters in this module exist.

use serde::{Deserialize, Serialize};

/// Tool descriptor sent in the Messages API request body.
///
/// ```json
/// {
///   "name": "write_file",
///   "description": "Write a file to disk.",
///   "input_schema": { ...JSON Schema... }
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct AnthropicToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// One block in an outgoing assistant message's `content` array.
/// Internally tagged on `type`, mirroring Anthropic's wire shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// One block in an outgoing user message's `content` array. Includes
/// the `tool_result` block used to reply to a prior assistant
/// `tool_use`. `Text` is part of Anthropic's documented user-content
/// schema (a user message can interleave prose with tool_results when
/// replying to multiple calls) but the orchestrator doesn't emit
/// that pattern today; mark it allowed-dead so the variant stays
/// available for the eventual case.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContentBlock {
    #[allow(dead_code)]
    Text { text: String },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "is_false")]
        is_error: bool,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One block in the incoming response's `content` array. Deserializes
/// `type` into a string we match on at parse time rather than a
/// rigid enum -- Anthropic adds new block types (thinking, redacted,
/// signature) and an untagged enum would reject them.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseContentBlock {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_descriptor_uses_input_schema_not_parameters() {
        // Pin Anthropic's required key name -- it's `input_schema`,
        // not `parameters` (which is the OpenAI key). A future
        // refactor that mistakenly renames this would silently
        // make Anthropic ignore our tools.
        let t = AnthropicToolDescriptor {
            name: "list_dir".into(),
            description: "List a directory".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        };
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["name"], "list_dir");
        assert_eq!(json["description"], "List a directory");
        assert_eq!(json["input_schema"]["type"], "object");
        assert!(json.get("parameters").is_none(), "json: {json}");
    }

    #[test]
    fn assistant_tool_use_block_serializes_with_input_object() {
        // Anthropic's `tool_use` blocks use `input` (a JSON object),
        // not OpenAI's stringified `arguments`. The orchestrator
        // converter parses our AdvertisedToolCall.arguments_json
        // into a serde_json::Value before emitting this block.
        let b = AssistantContentBlock::ToolUse {
            id: "toolu_abc".into(),
            name: "write_file".into(),
            input: serde_json::json!({"path": "docs/spec.md", "content": "..."}),
        };
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["id"], "toolu_abc");
        assert_eq!(json["name"], "write_file");
        assert_eq!(json["input"]["path"], "docs/spec.md");
    }

    #[test]
    fn user_tool_result_block_pairs_with_tool_use_id() {
        let b = UserContentBlock::ToolResult {
            tool_use_id: "toolu_abc".into(),
            content: "ok: 12 entries".into(),
            is_error: false,
        };
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "toolu_abc");
        assert_eq!(json["content"], "ok: 12 entries");
        // `is_error: false` is skipped on the wire.
        assert!(json.get("is_error").is_none(), "json: {json}");
    }

    #[test]
    fn user_tool_result_emits_is_error_when_true() {
        let b = UserContentBlock::ToolResult {
            tool_use_id: "toolu_abc".into(),
            content: "tool failed".into(),
            is_error: true,
        };
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["is_error"], true);
    }

    #[test]
    fn response_block_parses_text_and_tool_use_shapes() {
        let raw = r#"[
            {"type": "text", "text": "I'll list it."},
            {"type": "tool_use", "id": "toolu_1", "name": "list_dir", "input": {"path": "."}}
        ]"#;
        let blocks: Vec<ResponseContentBlock> = serde_json::from_str(raw).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind.as_deref(), Some("text"));
        assert_eq!(blocks[0].text.as_deref(), Some("I'll list it."));
        assert_eq!(blocks[1].kind.as_deref(), Some("tool_use"));
        assert_eq!(blocks[1].id.as_deref(), Some("toolu_1"));
        assert_eq!(blocks[1].name.as_deref(), Some("list_dir"));
        assert_eq!(blocks[1].input.as_ref().unwrap()["path"], ".");
    }

    #[test]
    fn response_block_tolerates_unknown_kinds() {
        // Anthropic adds new block kinds (thinking, signature,
        // redacted_thinking) over time. The deserializer must not
        // reject them outright; downstream code can ignore the
        // ones it doesn't care about.
        let raw = r#"[
            {"type": "thinking", "thinking": "..."},
            {"type": "redacted_thinking"}
        ]"#;
        let blocks: Vec<ResponseContentBlock> = serde_json::from_str(raw).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind.as_deref(), Some("thinking"));
        assert_eq!(blocks[1].kind.as_deref(), Some("redacted_thinking"));
    }
}
