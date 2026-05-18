//! Tool invocation + argument-parsing helpers.
//!
//! Everything the turn loop needs to take a `ParsedToolCall`, look
//! it up in the dispatcher, parse its args (JSON object form or the
//! per-tool line-based fallback), and cap the result string. Also
//! the small ancillary helpers that share the same scope: the
//! base64 encoder for image attachments, the `tool_args_summary`
//! one-liner for host events, the `run_phase_validator` shim around
//! `runners`, and the `SIM_FLOW_TOOL_MODE` resolver.

use std::path::Path;

use crate::session::runners;
use crate::session::tools::{self, Tool, ToolResult};

pub(super) fn invoke_tool(
    dispatcher: &[Box<dyn Tool>],
    ctx: &tools::ToolContext,
    call: &tools::ParsedToolCall,
) -> ToolResult {
    let tool = match dispatcher.iter().find(|t| t.name() == call.name) {
        Some(t) => t,
        None => {
            return ToolResult::err(format!(
                "tool `{}` is not available for this step",
                call.name
            ));
        }
    };
    let args = match tool_args_from_body(&call.name, &call.body) {
        Ok(v) => v,
        Err(msg) => return ToolResult::err(msg),
    };
    let result = match tool.invoke(ctx, &args) {
        Ok(out) => out,
        Err(err) => ToolResult::err(format!("tool `{}` failed: {err}", call.name)),
    };
    cap_tool_output(result)
}

/// Defensive global cap on tool output size. Most tools already
/// self-truncate (`read_file` at 16 KB, `run_cargo` at its own
/// tail-trim threshold), but a chatty / pathological tool result
/// that bypasses those still dominates the prompt stack. The cap
/// here is a defence-in-depth backstop; tools that already
/// trimmed produce output well under this threshold and aren't
/// touched.
const TOOL_OUTPUT_CAP_BYTES: usize = 16 * 1024;

fn cap_tool_output(mut result: ToolResult) -> ToolResult {
    if result.display.len() > TOOL_OUTPUT_CAP_BYTES {
        let head = &result.display[..TOOL_OUTPUT_CAP_BYTES];
        // Avoid splitting in the middle of a UTF-8 multi-byte
        // sequence: walk back to the last char boundary at or
        // before the cap.
        let cut = head
            .char_indices()
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let original_len = result.display.len();
        let mut trimmed = result.display[..cut].to_string();
        trimmed.push_str(&format!(
            "\n... (truncated by orchestrator output cap; original {original_len} bytes)"
        ));
        result.display = trimmed;
    }
    result
}

pub(super) fn tool_args_from_body(
    name: &str,
    body: &str,
) -> std::result::Result<serde_json::Value, String> {
    // JSON body is the universal shape: backends with native tool-use
    // (LM Studio function calling, OpenAI tool_calls, Anthropic
    // tool_use) synthesize fenced blocks whose body is the call's
    // arguments JSON, and `edit_file`'s multi-line strings already
    // require it. If the body parses as a JSON object we use it
    // directly; otherwise we fall back to the per-tool line-based
    // form documented in the system-prompt examples.
    //
    // `write_file` accepts JSON args here too: the system prompt
    // still recommends the artifact-write convention (fenced block
    // whose info-string is the file path) because it round-trips
    // cleanly through fenced-block-only backends, but rejecting
    // `tool:write_file` outright deadlocks native-tool-calling
    // backends — they synthesize `tool:<name>` fences for every
    // function-call response, and an unrecoverable rejection sends
    // them into a runaway retry loop until `max_identical_responses`
    // fires.
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        return match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) => Ok(value),
            Err(e) => Err(format!("{name}: failed to parse JSON args: {e}")),
        };
    }

    match name {
        "read_file" | "list_dir" => {
            let path = body
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string());
            match path {
                Some(p) => Ok(serde_json::json!({ "path": p })),
                None => Err(format!(
                    "{name}: empty body; expected a path on the first line"
                )),
            }
        }
        "search" => {
            let mut iter = body.lines().filter(|l| !l.trim().is_empty());
            let pattern = iter.next().map(|l| l.trim().to_string());
            let path = iter.next().map(|l| l.trim().to_string());
            match pattern {
                Some(p) => match path {
                    Some(scope) => Ok(serde_json::json!({ "pattern": p, "path": scope })),
                    None => Ok(serde_json::json!({ "pattern": p })),
                },
                None => Err("search: empty body; expected a regex pattern".into()),
            }
        }
        "edit_file" => Err(
            "edit_file fenced fallback requires a JSON object body, e.g. \
             `{\"path\": \"foo.md\", \"old_string\": \"...\", \"new_string\": \"...\"}`. \
             Prefer native tool-use when the backend supports it."
                .into(),
        ),
        "delete_file" => {
            // Single-arg tool: accept either the bare path on the
            // first non-empty line, or a JSON `{ "path": "..." }`
            // body (already handled by `parse_json_tool_block`).
            let path = body
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string());
            match path {
                Some(p) if !p.is_empty() => Ok(serde_json::json!({ "path": p })),
                _ => Err("delete_file: empty body; expected a single relative path".into()),
            }
        }
        "write_file" => {
            // Permissive fallback: treat the fenced body as
            // "path on the first non-empty line, content as the
            // rest" so an agent that reaches for `tool:write_file`
            // (the natural function-call shape) doesn't get
            // bounced when the body isn't JSON-wrapped. The JSON
            // path above still works; this branch covers backends
            // that emit bare path + content lines.
            let mut lines = body.lines();
            let path = loop {
                match lines.next() {
                    Some(l) if !l.trim().is_empty() => break Some(l.trim().to_string()),
                    Some(_) => continue,
                    None => break None,
                }
            };
            let Some(path) = path else {
                return Err(write_file_help("empty body"));
            };
            // Drop the leading blank line(s) commonly written
            // between the path and the content block.
            let mut content_lines: Vec<&str> = lines.collect();
            while content_lines.first().is_some_and(|l| l.trim().is_empty()) {
                content_lines.remove(0);
            }
            if content_lines.is_empty() {
                return Err(write_file_help(&format!(
                    "missing file content for `{path}`"
                )));
            }
            let content = content_lines.join("\n");
            Ok(serde_json::json!({ "path": path, "content": content }))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Helper text rendered when a `tool:write_file` fenced call lacks
/// the path-then-content body. Includes a concrete artifact-write
/// example so the agent can recover in one read instead of
/// trial-and-error.
fn write_file_help(reason: &str) -> String {
    format!(
        "write_file: {reason}. The fenced-tool body must be \"path on \
         line 1, blank line, content below\". For multi-line content \
         prefer the artifact-write convention -- the fence info-string \
         is the file path and the body is the file content:\n\n\
         ```src/model/mod.rs\npub mod foo;\npub mod bar;\n```\n\n\
         Or pass JSON args directly: \
         `{{\"path\": \"<rel>\", \"content\": \"<text>\"}}`."
    )
}

pub(super) fn tool_args_summary(call: &tools::ParsedToolCall) -> String {
    // edit_file is special-cased: the default 80-char first-line
    // truncation hides `old_string` and `new_string` entirely
    // (JSON serialization order varies, and either field can be
    // multiline). Without seeing both strings, a "successful" edit
    // that doesn't actually fix the offending content is invisible
    // in the host stream. Render path + old + new with generous
    // per-field caps so loop debugging doesn't require re-running
    // with extra instrumentation.
    if call.name == "edit_file"
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&call.body)
    {
        let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("?");
        let old = v.get("old_string").and_then(|x| x.as_str()).unwrap_or("");
        let new_s = v.get("new_string").and_then(|x| x.as_str()).unwrap_or("");
        return format!(
            "path={} old={} new={}",
            tools::preview_one_line(path, 120),
            tools::preview_one_line(old, 240),
            tools::preview_one_line(new_s, 240),
        );
    }
    let line = call.body.lines().next().unwrap_or("").trim();
    let mut iter = line.chars();
    let head: String = iter.by_ref().take(80).collect();
    if iter.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

/// Standard base64 (RFC 4648) encoder. Inlined to avoid pulling in
/// the `base64` crate just for the tool-attachment hand-off; we have
/// at most one or two image encodings per session.
pub(super) fn base64_encode(input: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n =
            (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8) | u32::from(input[i + 2]);
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHA[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = u32::from(input[i]) << 16;
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8);
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

pub(super) fn run_phase_validator(
    phase: &str,
    project_dir: &Path,
) -> Option<runners::RunnerOutput> {
    match phase {
        "build" => runners::cargo_check(project_dir).ok(),
        "test" => runners::cargo_test(project_dir, None).ok(),
        _ => None,
    }
}

pub(super) fn tool_call_persists_output(tool_name: &str) -> bool {
    matches!(tool_name, "write_file" | "edit_file" | "delete_file")
}

/// Resolve whether the orchestrator should advertise + dispatch its
/// native tool catalog. The historical default was fenced-mode (the
/// model emits ` ```tool:write_file ` fenced blocks and the
/// orchestrator parses them); native mode required opt-in via
/// `SIM_FLOW_TOOL_MODE=native`. Production runs have used native
/// almost exclusively (fenced mode burns turns on parsing-grade
/// near-misses with weaker open models), so the default flipped:
///
/// - **default / `native` / `native-tool-calls`**: native dispatch.
/// - **`fenced` / `fenced-blocks` / `off`**: fall back to fenced mode.
/// - **anything else**: native dispatch (unknown tokens default to
///   the new safe behavior; an explicit `fenced` is needed to opt
///   out).
///
/// One place to keep the gate so the host-side dispatch decision
/// (see `TerminalHost::request_llm_response`) and the prompt-template
/// fragment selection (see `build_initial_messages`) agree.
pub(crate) fn resolve_native_tool_mode() -> bool {
    let raw = std::env::var("SIM_FLOW_TOOL_MODE").ok();
    let Some(value) = raw.as_deref() else {
        return true;
    };
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "fenced" | "fenced-blocks" | "off" | "0" | "false"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_args_from_body_accepts_write_file_json() {
        // Regression: native-tool-calling backends (LM Studio, OpenAI,
        // Anthropic) translate function-call responses into
        // `tool:<name>` fenced blocks with JSON args. Rejecting
        // `write_file` outright sent those agents into a runaway
        // retry loop because they had no other shape to emit.
        let body = "{\"path\":\"docs/targets.md\",\"content\":\"# Targets\\n\"}";
        let value = tool_args_from_body("write_file", body)
            .expect("write_file with JSON args must be accepted");
        assert_eq!(
            value.get("path").and_then(|v| v.as_str()),
            Some("docs/targets.md")
        );
        assert_eq!(
            value.get("content").and_then(|v| v.as_str()),
            Some("# Targets\n")
        );
    }

    #[test]
    fn tool_args_from_body_rejects_write_file_without_content() {
        // A bare path line with nothing after it has no content to
        // write. The fallback must surface a help message including
        // a concrete artifact-write example so the agent can recover
        // in one turn instead of guessing.
        let err = tool_args_from_body("write_file", "docs/targets.md\n").unwrap_err();
        assert!(
            err.contains("missing file content"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("```src/model/mod.rs"),
            "expected example: {err}"
        );
        assert!(err.contains("artifact-write"), "expected guidance: {err}");
    }

    #[test]
    fn tool_args_from_body_accepts_write_file_path_then_content() {
        // Permissive fallback: agents that emit `tool:write_file`
        // with a bare path + blank line + content body get treated
        // as if they had passed JSON args. Native-tool-use backends
        // synthesize this shape constantly; rejecting it cost
        // auto-iters in the e2e flow. `body.lines()` strips trailing
        // newlines, so the joined content matches that view.
        let body = "src/model/mod.rs\n\npub mod payloads;\npub mod stages;\n";
        let value =
            tool_args_from_body("write_file", body).expect("path + content body must be accepted");
        assert_eq!(
            value.get("path").and_then(|v| v.as_str()),
            Some("src/model/mod.rs")
        );
        assert_eq!(
            value.get("content").and_then(|v| v.as_str()),
            Some("pub mod payloads;\npub mod stages;")
        );
    }

    #[test]
    fn tool_args_from_body_accepts_write_file_path_with_no_blank_separator() {
        // Some agents skip the blank line between path and content.
        // The fallback should still recover -- treat the rest of the
        // body as content directly.
        let body = "src/lib.rs\nfn main() {}\n";
        let value = tool_args_from_body("write_file", body)
            .expect("path + immediate content must be accepted");
        assert_eq!(
            value.get("path").and_then(|v| v.as_str()),
            Some("src/lib.rs")
        );
        assert_eq!(
            value.get("content").and_then(|v| v.as_str()),
            Some("fn main() {}")
        );
    }

    #[test]
    fn tool_args_from_body_rejects_write_file_with_only_blank_lines() {
        // Path line followed only by blank lines = no actual content
        // to write. Surface the same help message as an empty body.
        let err = tool_args_from_body("write_file", "src/foo.rs\n\n\n").unwrap_err();
        assert!(
            err.contains("missing file content"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn tool_args_summary_truncates_on_char_boundary() {
        // Regression: byte-index slicing panicked when the cut point
        // fell inside a multi-byte UTF-8 character. A DM4b critique
        // search whose pattern contained a literal emoji crashed the
        // orchestrator at the 80-byte cut. Truncation must count
        // chars, not bytes.
        // 3 ASCII bytes + 4-byte emojis: byte 80 lands inside emoji 20
        // (bytes 79..83), which is precisely the kind of cut that
        // crashed the byte-slice version.
        let body = format!("aaa{}bbbb", "\u{1F600}".repeat(100));
        let call = tools::ParsedToolCall {
            name: "search".into(),
            body,
        };
        let summary = tool_args_summary(&call);
        assert!(summary.ends_with("..."), "should truncate: {summary}");
    }

    #[test]
    fn base64_encode_handles_every_padding_arm() {
        // Empty input.
        assert_eq!(base64_encode(b""), "");
        // 1 byte -> 2 padding chars.
        assert_eq!(base64_encode(b"a"), "YQ==");
        // 2 bytes -> 1 padding char.
        assert_eq!(base64_encode(b"ab"), "YWI=");
        // 3 bytes -> no padding.
        assert_eq!(base64_encode(b"abc"), "YWJj");
        // Round-trip via base64 crate-equivalent canonical encoding:
        // "Hello World" -> "SGVsbG8gV29ybGQ=".
        assert_eq!(base64_encode(b"Hello World"), "SGVsbG8gV29ybGQ=");
    }

    #[test]
    fn tool_call_persists_output_only_for_write_tools() {
        assert!(tool_call_persists_output("write_file"));
        assert!(tool_call_persists_output("edit_file"));
        assert!(!tool_call_persists_output("read_file"));
        assert!(!tool_call_persists_output("search"));
        assert!(!tool_call_persists_output("run_cargo"));
    }

    #[test]
    fn tool_call_persists_output_only_for_mutating_tools() {
        for name in ["write_file", "edit_file", "delete_file"] {
            assert!(tool_call_persists_output(name), "{name}");
        }
        for name in ["read_file", "list_dir", "search", "run_cargo", "log_bug"] {
            assert!(!tool_call_persists_output(name), "{name}");
        }
    }

    #[test]
    fn resolve_native_tool_mode_defaults_to_native_when_env_unset() {
        // SAFETY: tests run with serial env mutation; this matches the
        // existing test-suite pattern.
        // Snapshot + restore so we don't bleed state to other tests.
        let prior = std::env::var("SIM_FLOW_TOOL_MODE").ok();
        unsafe {
            std::env::remove_var("SIM_FLOW_TOOL_MODE");
        }
        assert!(resolve_native_tool_mode());
        for v in [
            "native",
            "native-tool-calls",
            "NATIVE",
            " native ",
            "anything",
        ] {
            unsafe {
                std::env::set_var("SIM_FLOW_TOOL_MODE", v);
            }
            assert!(resolve_native_tool_mode(), "{v}");
        }
        for v in ["fenced", "fenced-blocks", "off", "0", "false", "OFF"] {
            unsafe {
                std::env::set_var("SIM_FLOW_TOOL_MODE", v);
            }
            assert!(!resolve_native_tool_mode(), "{v}");
        }
        // Restore.
        unsafe {
            match prior {
                Some(v) => std::env::set_var("SIM_FLOW_TOOL_MODE", v),
                None => std::env::remove_var("SIM_FLOW_TOOL_MODE"),
            }
        }
    }
}
