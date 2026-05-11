//! Native OpenAI tool-calls wire shape for the openai_compat
//! transport.
//!
//! OpenAI's chat-completions API accepts a `tools: [...]` parameter
//! describing the function catalog the model can call, and returns
//! `choices[0].message.tool_calls: [{id, type, function: {name,
//! arguments}}]` when the model decided to call one. vLLM with
//! `--enable-auto-tool-choice --tool-call-parser qwen3_coder`
//! translates the model's qwen-coder XML output into this same OpenAI
//! tool_calls shape transparently -- our client sees the standard
//! OpenAI representation regardless of how the model emits the call.
//!
//! These types are wire-only. The orchestrator converts them into the
//! agent-side `ParsedToolCall` shape and dispatches via the existing
//! tool registry.

use serde::{Deserialize, Serialize};

/// Tool descriptor sent in the chat-completions request body. Mirrors
/// OpenAI's `Tool` object shape:
///
/// ```json
/// {
///   "type": "function",
///   "function": {
///     "name": "write_file",
///     "description": "Write a file to disk.",
///     "parameters": { ...JSON Schema... }
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function"
    pub function: FunctionDescriptor,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionDescriptor {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema for arguments
}

impl ToolDescriptor {
    pub fn function(name: String, description: String, parameters: serde_json::Value) -> Self {
        Self {
            kind: "function",
            function: FunctionDescriptor {
                name,
                description,
                parameters,
            },
        }
    }
}

/// Native tool call parsed from `choices[0].message.tool_calls[N]`.
/// `arguments` is the raw JSON string the model emitted; downstream
/// parsing into a `serde_json::Value` happens at the orchestrator
/// boundary so the transport stays decoupled from the tool catalog.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct NativeToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub function: NativeToolFunction,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct NativeToolFunction {
    pub name: String,
    /// Per the OpenAI spec, `arguments` is a JSON-encoded string (NOT
    /// a JSON object). vLLM, OpenAI, and every conforming backend
    /// follow this. Keep it as a string here and parse at the
    /// orchestrator so a malformed payload surfaces a clear
    /// diagnostic rather than a cryptic serde error mid-pipeline.
    pub arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_descriptor_serializes_to_openai_shape() {
        // Pin the exact wire shape so a refactor can't silently
        // rename keys and break vLLM acceptance.
        let t = ToolDescriptor::function(
            "list_dir".to_string(),
            "List a directory".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        );
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "list_dir");
        assert_eq!(json["function"]["description"], "List a directory");
        assert_eq!(json["function"]["parameters"]["type"], "object");
        assert_eq!(json["function"]["parameters"]["required"][0], "path");
    }

    #[test]
    fn native_tool_call_parses_openai_response_shape() {
        // The exact shape vLLM returns when --enable-auto-tool-choice
        // and --tool-call-parser qwen3_coder are on. Pin it so a
        // future serde version or rename can't silently break the
        // parse.
        let raw = r#"{
            "id": "call_abc123",
            "type": "function",
            "function": {
                "name": "list_dir",
                "arguments": "{\"path\":\".\"}"
            }
        }"#;
        let parsed: NativeToolCall = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.id.as_deref(), Some("call_abc123"));
        assert_eq!(parsed.kind.as_deref(), Some("function"));
        assert_eq!(parsed.function.name, "list_dir");
        assert_eq!(parsed.function.arguments, r#"{"path":"."}"#);
    }

    #[test]
    fn native_tool_call_tolerates_missing_id_and_kind() {
        // Some openai-compat proxies omit `id` or `type` from the
        // tool_calls entries. The minimum viable shape is `function:
        // {name, arguments}` -- accept that without erroring.
        let raw = r#"{ "function": { "name": "read_file", "arguments": "{}" } }"#;
        let parsed: NativeToolCall = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.id, None);
        assert_eq!(parsed.kind, None);
        assert_eq!(parsed.function.name, "read_file");
    }
}
