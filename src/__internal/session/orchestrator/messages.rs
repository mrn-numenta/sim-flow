//! Initial prompt + per-session input assembly.
//!
//! Produces the message stack the orchestrator ships to the LLM on
//! the first turn (system prompt, tool notice, project TOCs,
//! milestone-scope preamble, inlined critique body, opening user
//! turn) and the protocol-side `StepDescriptorOut` that goes in the
//! HelloAck. The interactive PTY driver and the JSONL turn loop both
//! call `build_initial_messages`, so this module is the
//! single-source-of-truth for what the agent sees first.

use std::path::Path;

use crate::Result;
use crate::client::SessionKind;
use crate::prompts;
use crate::session::protocol::{LlmMessage, LlmRole, LlmTool, SessionKindOut, StepDescriptorOut};
use crate::session::tools::{self, Tool};
use crate::steps::StepDescriptor;

use super::artifacts::{detect_framework_docs_root, detect_framework_root, detect_library_root};
use super::gates::retry_gate_finding_blocks;
use super::options::OrchestratorOptions;
use super::tools_dispatch::resolve_native_tool_mode;

/// Render a system message describing the fenced-block tool-call
/// fallback that backends without native tool-use can emit. Native
/// tool-use clients still see the same tools via the protocol's
/// `RequestLlmResponse.tools` field.
pub(super) fn build_tool_notice(
    dispatcher: &[Box<dyn Tool>],
    library_root: Option<&Path>,
    framework_root: Option<&Path>,
    framework_docs_root: Option<&Path>,
    write_paths: &[String],
    native_mode: bool,
) -> String {
    let mut out = String::new();
    // In native mode the API also receives the tool catalog via the
    // `tools` request field, so re-describing each tool here is
    // duplication that wastes attention budget. Drop the listing
    // and the fenced-syntax tutorial below; just keep the
    // orchestrator-specific info (write scope, library / framework
    // roots) that ISN'T conveyed elsewhere.
    if !native_mode {
        out.push_str("Tool catalog (orchestrator-mediated):\n\n");
        for t in dispatcher {
            out.push_str(&format!("- `{}` - {}\n", t.name(), t.description()));
        }
    }
    let reject_clause = if native_mode {
        "Paths outside this list are rejected by `write_file` and `edit_file`. If you have a strong reason to land work elsewhere, surface it in your reply rather than retrying with a different out-of-scope path."
    } else {
        "Paths outside this list are rejected by `write_file`, `edit_file`, AND the fenced ` ```<path> ` artifact-write convention. If you have a strong reason to land work elsewhere, surface it in your reply rather than retrying with a different out-of-scope path."
    };
    let disabled_clause = if native_mode {
        "Writes are disabled in this session. `write_file` and `edit_file` will reject any path. Use the read-only tools to inspect state and report findings as text."
    } else {
        "Writes are disabled in this session. `write_file`, `edit_file`, and the fenced artifact-write convention will all reject any path. Use the read-only tools to inspect state and report findings as text."
    };
    if write_paths.is_empty() {
        out.push_str(&format!("\n{disabled_clause}\n"));
    } else {
        out.push_str(
            "\nWrite scope (per step + kind): the orchestrator only persists writes that match one of these project-relative prefixes (entries ending in `/` match any path under that directory; others must match exactly):\n",
        );
        for p in write_paths {
            out.push_str(&format!("- `{p}`\n"));
        }
        out.push_str(&format!("{reject_clause}\n"));
    }
    if let Some(root) = library_root {
        out.push_str(&format!(
            "\nLibrary root (read-only, auto-detected): `{}`. Reads can target it by prefixing the path with `lib:`; for example `lib:docs/modeling-guide/01-quickstart.md` or `lib:examples/00-simple-pipeline/`. `list_dir` accepts a bare `lib:` to list the library root itself. `write_file` rejects `lib:` paths -- writes always land under the project directory.\n",
            root.display()
        ));
    } else {
        out.push_str(
            "\nNo library root detected. `lib:` reads will fail until a sim-models layout is found above the project dir.\n",
        );
    }
    if let Some(root) = framework_root {
        out.push_str(&format!(
            "\nFramework source root (read-only): `{}`. Reads can target it via the `fw:` prefix for source-level signatures and crate layout. Prefer the curated rustdoc under `fw:api/...` for API discovery; use `fw:src/prelude.rs` or individual `fw:src/...` files only when you need exact signatures or source examples. Treat the framework as a stable API -- do NOT browse internal helpers; if a behavior isn't in the prelude or a directly-re-exported module, ask rather than reverse-engineering it.\n",
            root.display()
        ));
    }
    if let Some(root) = framework_docs_root {
        out.push_str(&format!(
            "\nFramework API docs root (read-only): `{}`. A curated framework API TOC is provided separately in this prompt. Use that TOC to choose specific `fw:api/pages/...md` files, then read only those pages on demand.\n",
            root.display()
        ));
    }
    // Fenced-style tool-use tutorial. Only relevant when the model
    // is going to emit ` ```tool:<name> ` blocks rather than native
    // function calls. In native mode the API's `tools` parameter
    // already describes the catalog with proper JSON schemas, so
    // this entire tutorial is dead weight (and the example fences
    // can actively confuse a model that's been told NOT to emit
    // fences in the orchestrator-native-tools convention).
    if !native_mode {
        out.push_str(
            "\nNative tool-use is preferred; clients without it can emit a fenced block whose info-string is `tool:<name>` and whose body is the argument payload. Examples:\n\n```tool:read_file\nsrc/lib.rs\n```\n\n```tool:list_dir\nfw:\n```\n\n```tool:read_file\nfw:api/toc.md\n```\n\n```tool:read_file\nfw:api/pages/foundation_framework/prelude/index.md\n```\n\n```tool:read_file\nfw:src/prelude.rs\n```\n\n```tool:search\n{\"pattern\":\"ConnectivityPlan\",\"path\":\"fw:api/pages\"}\n```\n\nThe `edit_file` tool's fenced-block body is a JSON object (its three args -- `path`, `old_string`, `new_string` -- can be multi-line, so a JSON body is the only unambiguous form):\n\n```tool:edit_file\n{\"path\": \"spec.md\", \"old_string\": \"## Pipelining\", \"new_string\": \"## Pipelining and Hierarchy\"}\n```\n\n## Choosing between edit_file and the artifact-write convention\n\nPrefer `edit_file` for SMALL, TARGETED CHANGES against a file already on disk: rename a header, fix a typo, change a single value, add or delete a paragraph. `old_string` must appear EXACTLY ONCE in the current file -- include enough surrounding context to make the substring unique, and read the file first if you don't already have its current text in this turn. Use the artifact-write convention (full-file fenced block whose info-string is the path) only when creating a new file or when the change touches most of the file.\n\nThe orchestrator runs the tool, emits a `ToolInvoked` event for the host, and feeds the tool's output back as the next user message.",
        );
    } else {
        // Native mode: a tight one-liner reminding the model when
        // edit_file is preferred over write_file, since the API's
        // tool descriptions don't convey that nuance.
        out.push_str(
            "\nPrefer `edit_file` for SMALL, TARGETED CHANGES against a file already on disk (rename a header, fix a typo, change a single value). `old_string` must appear EXACTLY ONCE in the current file -- include enough surrounding context to make the substring unique, and call `read_file` first if you don't already have its current text in this turn. Use `write_file` for new files or when the change touches most of the file.",
        );
    }
    out
}

pub(crate) fn step_descriptor_for_protocol(
    step: &StepDescriptor,
    kind: SessionKindOut,
    foundation_root: &Path,
) -> StepDescriptorOut {
    let suffix = match kind {
        SessionKindOut::Work => "",
        SessionKindOut::Critique => "-critique",
        // Q&A turns don't have a per-step instruction prompt --
        // they're driven by `run_manual_qa_turn` and never go
        // through `step_descriptor_for_protocol`. If we ever land
        // here for Qa, it's a logic bug worth surfacing.
        SessionKindOut::Qa => unreachable!(
            "step_descriptor_for_protocol called with SessionKindOut::Qa; \
             Q&A turns don't use per-step descriptors"
        ),
    };
    let path = foundation_root
        .join(crate::prompts::PROMPTS_DIR)
        .join(format!("{}{}.md", step.instruction_slug, suffix));
    let (phases, tool_names) = match kind {
        SessionKindOut::Work => (step.work_phases, crate::steps::UNIVERSAL_TOOLS),
        SessionKindOut::Critique => (step.critique_phases, crate::steps::UNIVERSAL_TOOLS),
        SessionKindOut::Qa => unreachable!(
            "step_descriptor_for_protocol Qa branch unreachable -- see suffix arm above"
        ),
    };
    StepDescriptorOut {
        step: step.id.into(),
        kind,
        flow: step.flow.as_str().into(),
        prerequisite: step.prerequisite.map(String::from),
        instruction_path: path.display().to_string(),
        work_artifacts: step.work_artifacts.iter().map(|s| (*s).into()).collect(),
        predecessor_inputs: step
            .predecessor_inputs
            .iter()
            .map(|s| (*s).into())
            .collect(),
        per_candidate: step.per_candidate,
        phases: phases.iter().map(|s| (*s).into()).collect(),
        tools: tool_names.iter().map(|s| (*s).into()).collect(),
    }
}

// ---------------------------------------------------------------------
// Public message-building entry point used by both the JSONL turn loop
// and the interactive PTY driver. Produces the exact stack of system /
// user messages the orchestrator would otherwise assemble inline at
// the start of `run_session_inner`, plus the advertised tool catalog.
// ---------------------------------------------------------------------

/// What `build_initial_messages` returns: the full message stack ready
/// to ship to an LLM (or render into a single prompt for an
/// interactive session) plus the tool catalog for backends with
/// native tool-use.
pub struct MessageBundle {
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<LlmTool>,
}

pub fn build_initial_messages(
    opts: &OrchestratorOptions,
    step: &StepDescriptor,
) -> Result<MessageBundle> {
    let tool_names: &[&'static str] = crate::steps::UNIVERSAL_TOOLS;
    let dispatcher = tools::build_dispatcher(tool_names);
    let library_root = detect_library_root(&opts.project_dir);
    let framework_root = detect_framework_root(&opts.foundation_root);
    let framework_docs_root = detect_framework_docs_root(&opts.foundation_root);
    let llm_tools: Vec<LlmTool> = dispatcher
        .iter()
        .map(|t| LlmTool {
            name: t.name().into(),
            description: t.description().into(),
            args_schema: t.args_schema(),
        })
        .collect();

    // Build the prompt-template context. The per-step prompts use
    // `{{output_intro}}` to defer the "how to emit artifacts"
    // preamble to a mode-specific fragment under `_templates/`. Same
    // gate the convention selection below uses (env var +
    // PTY/CLI flag); the per-step prompt directive must match the
    // session-wide convention or the model gets contradictory
    // instructions.
    let orchestrator_native_tools_mode =
        !opts.agent_has_native_fs_tools && resolve_native_tool_mode();
    let output_intro_fragment = if opts.agent_has_native_fs_tools {
        // PTY/CLI agents have their OWN tool catalog (Write, Edit,
        // Read). Today they share the fenced-mode preamble because
        // their tools cover the same use case via CLI-side syntax;
        // a CLI-specific intro fragment can be split out later if
        // needed.
        "output-intro-fenced"
    } else if orchestrator_native_tools_mode {
        "output-intro-native"
    } else {
        "output-intro-fenced"
    };
    let mut template_context = prompts::PromptContext::new();
    template_context.insert(
        "output_intro".into(),
        prompts::load_template(&opts.foundation_root, output_intro_fragment)?,
    );
    // `{{ step_id }}` substitutes to the step descriptor's id
    // (e.g. "DM0", "DM2cd"). Used inline by per-step prompts and
    // by the critique-json-schema fragment (which is pre-rendered
    // below so the prompt sees a finished string).
    template_context.insert("step_id".into(), step.id.to_string());
    // Pre-render the critique-json-schema fragment with the step
    // id so per-critique prompts can splice it in with a single
    // `{{ critique_json_schema }}` placeholder. The fragment
    // itself contains `{{ step_id }}` (Anthropic / OpenAI both
    // need the right step id in the example JSON body).
    let mut fragment_ctx = prompts::PromptContext::new();
    fragment_ctx.insert("step_id".into(), step.id.to_string());
    let critique_schema_body =
        prompts::load_template(&opts.foundation_root, "critique-json-schema")?;
    let critique_schema_rendered =
        prompts::render_prompt("critique-json-schema", &critique_schema_body, &fragment_ctx)?;
    template_context.insert(
        "critique_json_schema".into(),
        critique_schema_rendered.clone(),
    );
    // `{{ critique_output_block }}` — the shared "Write the
    // critique as JSON to docs/critiques/<step>-critique.json /
    // schema below" trailer every critique prompt ends with.
    // Pre-rendered (like `{{ critique_json_schema }}`) so the
    // step id AND the schema body land verbatim, since the
    // outer prompt render does not recurse into substituted
    // strings.
    let mut output_block_ctx = prompts::PromptContext::new();
    output_block_ctx.insert("step_id".into(), step.id.to_string());
    output_block_ctx.insert("critique_json_schema".into(), critique_schema_rendered);
    let output_block_body = prompts::load_template(&opts.foundation_root, "critique-output-block")?;
    let output_block_rendered = prompts::render_prompt(
        "critique-output-block",
        &output_block_body,
        &output_block_ctx,
    )?;
    template_context.insert("critique_output_block".into(), output_block_rendered);
    // `{{ third_party_reviewer_note }}` — the universal sentence
    // every critique prompt shares (independent-review framing).
    // No parameters; just a flat fragment.
    template_context.insert(
        "third_party_reviewer_note".into(),
        prompts::load_template(&opts.foundation_root, "third-party-reviewer")?,
    );
    // `{{ critique_kinds }}` — the canonical guidance every
    // critique prompt uses to introduce the `blocker` /
    // `unresolved` / `resolved` semantics. Centralised so that
    // future model-observed variants (`warning`, `issue`, ...)
    // can be ruled out in one place instead of touching every
    // per-step critique prompt. Bound unconditionally — work
    // prompts that never reference `{{ critique_kinds }}` simply
    // ignore the context entry, and the strict-undefined
    // renderer would error if a critique prompt forgot to splice
    // it in.
    template_context.insert(
        "critique_kinds".into(),
        prompts::load_template(&opts.foundation_root, "critique-kinds")?,
    );
    // `{{ coding_requirements }}` — the 6-bullet Rust style block
    // (idiomatic / data-oriented / functional / no magic literals /
    // no emojis / 400-line cap) every code-authoring DMF step
    // (DM2d, DM3b, DM3c, DM4b) inlines. Centralised so the rules
    // drift in exactly one place.
    template_context.insert(
        "coding_requirements".into(),
        prompts::load_template(&opts.foundation_root, "coding-requirements")?,
    );
    // `{{ coding_requirements_checks }}` — the critique-side
    // counterpart: a 4-bullet sub-list (4-space indented to nest
    // under a numbered evaluation row) covering idiomatic-Rust /
    // magic-literals / emojis / file-size-cap checks every
    // code-reviewing DMF critique (DM2d, DM3b, DM3c, DM4b) runs.
    template_context.insert(
        "coding_requirements_checks".into(),
        prompts::load_template(&opts.foundation_root, "coding-requirements-checks")?,
    );
    // `{{ pre_stop_hygiene }}` — the cargo-fmt-and-clippy
    // "orchestrator runs these after you stop" reminder every
    // code-authoring DMF step needs at end-of-milestone. The
    // template is top-level unindented; each prompt splices it
    // at flat indent.
    template_context.insert(
        "pre_stop_hygiene".into(),
        prompts::load_template(&opts.foundation_root, "pre-stop-hygiene")?,
    );
    // `{{ order_jumping_deferring }}` — the canonical
    // pointer-to-plan-management.md paragraph every milestone-
    // driven DMF step (DM2d, DM3b, DM3c, DM4b) needs near its
    // procedure. Step-specific deferred-row notes (DM3c's
    // coverage-threshold caveat, DM4b's target-row caveat) live
    // in the prompt right after the splice.
    template_context.insert(
        "order_jumping_deferring".into(),
        prompts::load_template(&opts.foundation_root, "order-jumping-deferring")?,
    );
    let instruction_body = prompts::load_for_project_with_context(
        &opts.foundation_root,
        &opts.project_dir,
        step.instruction_slug,
        opts.kind,
        &template_context,
    )?;
    let mut messages: Vec<LlmMessage> = Vec::new();
    // Boilerplate "conventions" (artifact-write rules, automated-mode
    // notes) live as files under `<foundation>/<PROMPTS_DIR>/_conventions/`.
    // Two delivery shapes:
    //   - Native-tools agents (interactive `claude` / `codex` /
    //     `gh-copilot`) get a thin bootstrap directive that names the
    //     absolute path; the agent's own Read tool fetches the body.
    //     Skipping the inline keeps a multi-thousand-character paste
    //     out of the PTY (paste-detection / ECHO / double-newline
    //     pain). Step-specific instructions stay inlined since
    //     they're small and we want them guaranteed in context.
    //   - JSONL hosts (no native Read) keep inlining; the orchestrator
    //     loads the same file from disk so the wording is single-
    //     source-of-truth.
    // Three artifact-write conventions today:
    //   - `native-tools`: PTY/CLI agents (claude / codex / gh-copilot)
    //     that have their OWN filesystem tools (Write / Edit / Read /
    //     Glob). Their tools land bytes directly on disk; the
    //     orchestrator just observes the gate state afterwards.
    //   - `orchestrator-native-tools`: HTTP backends running with
    //     `SIM_FLOW_TOOL_MODE=native` -- the orchestrator advertises
    //     its own tool catalog (`write_file`, `edit_file`, ...) over
    //     the OpenAI / Anthropic function-calling channel, runs the
    //     calls inside the orchestrator process, and feeds results
    //     back as Tool-role messages. The lowercased names + the
    //     "orchestrator runs the call" framing distinguish this from
    //     the CLI-side native-tools convention.
    //   - `fenced-blocks`: legacy path. The model emits a fenced
    //     markdown block whose info-string is the relative path; the
    //     orchestrator parses the fence and persists the body.
    //     Default when neither native path is active.
    //
    // `agent_has_native_fs_tools` is set by the PTY/interactive
    // driver; `SIM_FLOW_TOOL_MODE=native` is read above into
    // `orchestrator_native_tools_mode` for the template context.
    let convention_name = if opts.agent_has_native_fs_tools {
        "native-tools"
    } else if orchestrator_native_tools_mode {
        "orchestrator-native-tools"
    } else {
        "fenced-blocks"
    };
    // Mode-notes: always inject a positive signal for the current
    // mode (not the absence of the other one). Earlier we relied on
    // "auto-mode notes get loaded only when auto, the step prompt
    // does a self-check on the literal string." Weaker models
    // (qwen3-coder etc.) couldn't tell that a backtick-quoted
    // pattern reference was different from an active assertion, and
    // happily proceeded as if auto mode were on. Loading both
    // conventions side-by-side -- one per branch -- gives every
    // model an unambiguous "MANUAL mode is ACTIVE" / "AUTOMATED mode
    // is ACTIVE" anchor.
    let mode_notes_name = if opts.auto {
        "auto-mode"
    } else {
        "manual-mode"
    };
    let mode_notes_label = if opts.auto {
        "automated-mode notes"
    } else {
        "manual-mode notes"
    };
    // The test-loop auto-mode addendum (declare_fix accounting +
    // bug log) only applies to auto-mode sessions on steps that
    // actually run cargo build / cargo test. Chat-only DMF steps
    // (DM0/1/2a/2b/2c/2cd/3a/3ad/4a/4ad/4b) don't carry it.
    let test_loop_addendum_name: Option<&'static str> = if opts.auto
        && opts.kind == SessionKind::Work
        && step
            .work_phases
            .iter()
            .any(|p| *p == "test" || *p == "build")
    {
        Some("auto-mode-test-loop")
    } else {
        None
    };
    let combined_system = if opts.agent_has_native_fs_tools {
        let mut directives = format!(
            "Before responding, read the conventions file at:\n\n  {}\n\n\
             Treat its content as a system instruction that applies for\n\
             the rest of this session. The file is short (read it in full).\
             \n\nAlso read the {} at:\n\n  {}\n\nFollow them on every turn.\
             \n\nAlso read the no-emojis convention at:\n\n  {}\n\n\
             ASCII only -- no decorative glyphs in files, tool args, or chat replies.",
            prompts::convention_path(&opts.foundation_root, convention_name).display(),
            mode_notes_label,
            prompts::convention_path(&opts.foundation_root, mode_notes_name).display(),
            prompts::convention_path(&opts.foundation_root, "no-emojis").display(),
        );
        if let Some(addendum) = test_loop_addendum_name {
            directives.push_str(&format!(
                "\n\nAlso read the test-loop addendum at:\n\n  {}\n\n\
                 It covers cargo-test investigation / fix-attempt accounting and the bug log.",
                prompts::convention_path(&opts.foundation_root, addendum).display(),
            ));
        }
        if opts.no_preamble {
            directives.push_str(&format!(
                "\n\nAlso read the response-shape convention at:\n\n  {}\n\n\
                 Tool calls first, prose last. No recap, no hedging, no preamble.",
                prompts::convention_path(&opts.foundation_root, "no-preamble").display(),
            ));
        }
        format!("{}\n\n---\n\n{}", directives, instruction_body)
    } else {
        let convention = prompts::load_convention(&opts.foundation_root, convention_name)?;
        let mode_notes = prompts::load_convention(&opts.foundation_root, mode_notes_name)?;
        let no_emojis = prompts::load_convention(&opts.foundation_root, "no-emojis")?;
        let mut combined = format!(
            "{}\n\n---\n\n{}\n\n---\n\n{}\n\n---\n\n",
            convention, mode_notes, no_emojis,
        );
        if let Some(addendum) = test_loop_addendum_name {
            let test_loop_notes = prompts::load_convention(&opts.foundation_root, addendum)?;
            combined.push_str(&test_loop_notes);
            combined.push_str("\n\n---\n\n");
        }
        if opts.no_preamble {
            let no_preamble = prompts::load_convention(&opts.foundation_root, "no-preamble")?;
            combined.push_str(&no_preamble);
            combined.push_str("\n\n---\n\n");
        }
        combined.push_str(&instruction_body);
        combined
    };
    messages.push(LlmMessage {
        role: LlmRole::System,
        content: combined_system,
        attachments: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        reasoning: None,
    });
    if !llm_tools.is_empty() {
        let write_paths = crate::steps::allowed_write_paths(step, opts.kind);
        messages.push(LlmMessage {
            role: LlmRole::System,
            content: build_tool_notice(
                &dispatcher,
                library_root.as_deref(),
                framework_root.as_deref(),
                framework_docs_root.as_deref(),
                &write_paths,
                orchestrator_native_tools_mode,
            ),
            attachments: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
        });
    }
    // Stable-first ordering: project-stable TOCs (spec, framework
    // API), then per-step stable inputs, then per-milestone /
    // per-retry volatile inputs. vLLM's KV prefix cache reuses every
    // token-identical message at the head of the request, so anything
    // that changes between dispatches (current milestone, prior
    // critique body) goes LAST so the long stable head stays cached
    // across milestone advances and critique retries within a step.
    // The legacy `build_spec_toc_message` system message used to land
    // here, inlining `.sim-flow/source-spec-toc.md` and telling the
    // agent to `read_file` per-page chunks under
    // `.sim-flow/spec-pages/<NNN>.md`. With the format-discovery
    // pipeline owning source-spec retrieval (via the
    // `spec_semantic_search` tool over `.sim-flow/spec-ingest/`), no
    // analogous system message is needed: the per-step prompts
    // describe the corpus + retrieval tool, and the agent fetches
    // chunks lazily. Inlining a TOC here would (a) duplicate the
    // prompt guidance and (b) reintroduce the spec-pages reading
    // habit we just retired.
    if let Some(root) = framework_docs_root.as_deref()
        && let Some(toc) = build_framework_api_toc_message(root)
    {
        messages.push(LlmMessage {
            role: LlmRole::System,
            content: toc,
            attachments: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
        });
    }
    if let Some(inputs) = build_session_inputs(
        &opts.project_dir,
        step,
        opts.kind,
        opts.milestone_name.as_ref(),
    ) {
        messages.push(LlmMessage {
            role: LlmRole::System,
            content: inputs.stable,
            attachments: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
        });
        if let Some(volatile) = inputs.volatile {
            messages.push(LlmMessage {
                role: LlmRole::System,
                content: volatile,
                attachments: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning: None,
            });
        }
    }
    let opening = initial_user_prompt(
        step.id,
        opts.kind,
        &expected_output_paths(step, opts.kind),
        orchestrator_native_tools_mode,
    );
    messages.push(LlmMessage {
        role: LlmRole::User,
        content: opening,
        attachments: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        reasoning: None,
    });

    Ok(MessageBundle {
        messages,
        tools: llm_tools,
    })
}

// ---------------------------------------------------------------------
// Helpers shared with the (now-deleted) TS implementation. Behavioral
// parity with `extensions/sim-flow-vscode/src/participant/artifacts.ts`
// and `handlers.ts::initialUserPrompt` / `buildCritiqueInputs`.
// ---------------------------------------------------------------------

// AUTO_MODE_SYSTEM, ARTIFACT_CONVENTION_SYSTEM, and NATIVE_FS_TOOLS_SYSTEM
// used to live here as `concat!` strings. They were extracted to
// `<foundation>/tools/sim-flow/prompts/_conventions/{auto-mode,
// manual-mode,fenced-blocks,native-tools,no-emojis,no-preamble}.md` so:
//   - PTY agents that have a Read tool can fetch them on demand
//     instead of having a multi-thousand-character paste shoved into
//     stdin (avoiding paste-detection / ECHO / newline doubling).
//   - JSONL hosts still inline them, but via runtime read so there's
//     a single source of truth for the wording.
// `prompts::load_convention(foundation_root, name)` is the loader;
// `build_initial_messages` chooses inline vs reference-by-path based
// on `OrchestratorOptions::agent_has_native_fs_tools`.

pub(super) fn expected_output_paths(step: &StepDescriptor, kind: SessionKind) -> Vec<String> {
    match kind {
        SessionKind::Work => step.work_artifacts.iter().map(|s| (*s).into()).collect(),
        // Canonical critique form is JSON; the orchestrator renders
        // the `.md` sibling on disk after the agent writes the JSON
        // (see `write_artifact` and `write_file` tool). The agent
        // should NOT write the markdown directly, so we don't
        // surface its path here.
        SessionKind::Critique => vec![format!("docs/critiques/{}-critique.json", step.id)],
    }
}

fn initial_user_prompt(
    step_id: &str,
    kind: SessionKind,
    paths: &[String],
    native_mode: bool,
) -> String {
    let mut out = String::new();
    let critique_emit_clause = if native_mode {
        "Call `write_file` for the critique JSON file as specified by the instructions."
    } else {
        "The artifact-write block for the critique file as specified by the instructions."
    };
    let work_emit_clause = if native_mode {
        "Once you've read what you need, call `write_file` for the artifact file(s) -- or `edit_file` for targeted fixes -- as soon as you have enough content to save."
    } else {
        "Once you've read what you need, emit the artifact file(s) using the artifact-write convention -- or `edit_file` for targeted fixes -- as soon as you have enough content to save."
    };
    match kind {
        SessionKind::Work => {
            out.push_str(&format!(
                "Begin the {step_id} work session now. The TOC above lists this step's predecessor inputs and target artifacts (path + size only); fetch any of them with `read_file` before you make claims about their content. Your VERY FIRST RESPONSE must contain:\n\n\
                 1. The `read_file` tool calls you need to inspect target artifacts that are already on disk and any predecessor inputs that aren't yet covered by the inlined critique below; OR, if you've already read everything you need (e.g. a small step with only a critique inlined), one short sentence stating what each target artifact currently contains.\n\
                 2. Either:\n\
                    a. A bulleted list of what is still missing relative to the instructions / gate checks, followed by ONE concrete question for me about the most important missing item; OR\n\
                    b. The single line `All required content appears present - run /advance to gate-check.` if every item the instructions require is already covered.\n\n\
                 Do not return an empty response. Do not wait for further prompting. {work_emit_clause}",
            ));
        }
        SessionKind::Critique => {
            out.push_str(&format!(
                "Begin the {step_id} critique now. The TOC above lists this step's predecessor inputs and target artifacts (path + size only); fetch them with `read_file` before critiquing -- the content is NOT inlined. Your VERY FIRST RESPONSE must contain all three of:\n\n\
                 1. The `read_file` tool calls you need to inspect each target artifact and any predecessor input you'll cite; OR, once you've already read what you need this turn, a one-sentence summary of what the step's artifacts cover.\n\
                 2. A bulleted list of concrete issues you would flag relative to the step instructions and gate checks.\n\
                 3. {critique_emit_clause}\n\n\
                 Do not wait for further prompting; read what you need then emit the critique.",
            ));
        }
    }
    if !paths.is_empty() {
        if native_mode {
            out.push_str(
                "\n\nWrite these files by calling `write_file` (the `path` argument is the project-relative path; the `content` argument is the full file body):\n\n",
            );
        } else {
            out.push_str(
                "\n\nWrite these files using the artifact-write convention (fenced block with the path as the info-string):\n\n",
            );
        }
        for p in paths {
            out.push_str(&format!("- `{p}`\n"));
        }
        out.push_str("\nUse those exact paths - do NOT invent new filenames.");
    }
    out
}

/// If normalized framework API docs are available, return the bundled
/// TOC as a system message. The TOC points at `fw:api/pages/...` files
/// so the agent fetches only the specific API pages it needs.
fn build_framework_api_toc_message(framework_docs_root: &Path) -> Option<String> {
    let body = std::fs::read_to_string(framework_docs_root.join("toc.md")).ok()?;
    Some(format!(
        "Framework API docs are available under the `fw:api/` prefix. \
         Do NOT read the full API surface at once. Read the TOC below, then fetch only the \
         specific `fw:api/pages/...` files you need.\n\n{body}"
    ))
}

/// Split form of the per-session inputs message. `stable` is the
/// preamble + predecessor / work-artifact / plan-index TOC that does
/// NOT change across milestones or critique retries within a step.
/// `volatile` is the milestone-scope preamble + current-milestone TOC
/// entry + inlined critique body -- everything that DOES change. The
/// caller emits them as TWO separate System messages so vLLM's prefix
/// cache can reuse the long stable prefix across dispatches; without
/// the split the volatile tail invalidates the cache from the first
/// turn onward and the model re-encodes the entire input each time.
struct SessionInputs {
    stable: String,
    volatile: Option<String>,
}

fn build_session_inputs(
    project_dir: &Path,
    step: &StepDescriptor,
    kind: SessionKind,
    milestone_name: Option<&String>,
) -> Option<SessionInputs> {
    // Predecessors and this step's existing artifacts are listed as
    // a TOC (path + size) -- the agent fetches their content via
    // `read_file` on demand. This avoids burning tokens re-inlining
    // every predecessor on every turn of a long iteration loop. Two
    // exceptions that ARE inlined verbatim because they're the
    // immediate context the agent must act on:
    //
    //   - the active <step>-critique.md file on a work re-run
    //     (the findings the agent must address this turn);
    //   - the same file on a CRITIQUE re-run, to scope the second
    //     pass to "are the prior BLOCKERs resolved?" instead of
    //     repeating the full structural-question evaluation. Without
    //     this the second-pass critique re-derives every question
    //     from scratch and weaker models routinely flag new blockers
    //     that didn't exist in the first pass, blowing the
    //     critique-iteration budget.
    let critique_rel = format!("docs/critiques/{}-critique.md", step.id);
    let critique_abs = project_dir.join(&critique_rel);
    // Read the rendered markdown body for legacy fallback / first-
    // pass critique inlining; the JSON sibling (when present) is
    // the source of truth for gate-failing findings. `critique_body` is
    // None when neither artifact is on disk yet.
    let critique_body = std::fs::read_to_string(&critique_abs).ok();
    // Critique-retry detection: the file exists AND it's a critique
    // session AND we're not on the first pass. The first-pass test
    // is "no gate-failing findings at all" -- a fresh critique file from
    // a prior run that already evaluated cleanly wouldn't have any.
    // We guard on gate-failing finding presence so a previously-clean critique
    // doesn't suppress the full evaluation when the agent
    // legitimately needs it (e.g. the work session was edited
    // externally between runs).
    let prior_critique_gate_findings = retry_gate_finding_blocks(project_dir, step.id);
    let is_critique_retry =
        kind == SessionKind::Critique && !prior_critique_gate_findings.is_empty();
    let inline_critique = kind == SessionKind::Work || is_critique_retry;

    // Milestone-walk scoping. When a step's descriptor binds it to
    // a milestone-walk (DM2d, DM3b, DM3c, DM4b), the orchestrator
    // shows the agent ONLY the current milestone file plus the
    // plan's index, not the whole milestone directory. The
    // auto-driver iterates work-then-critique sessions; each
    // iteration the orchestrator picks the right milestone (same
    // one for retry, next pending one for advance). Without this
    // scoping the agent sees every milestone file at once and
    // chains them in a single work session, defeating the
    // per-milestone critique pattern.
    //
    // The current-milestone choice depends on session kind AND
    // retry state:
    //
    // - **Work, fresh advance** (no prior BLOCKERs): scope to the
    //   FIRST pending milestone -- the next slice of work.
    // - **Work, retry** (prior BLOCKERs): scope to the
    //   HIGHEST-numbered already-touched milestone -- the same
    //   milestone the Work session was on when the critique
    //   raised the BLOCKERs.
    // - **Critique** (any state): scope to the HIGHEST-numbered
    //   already-touched milestone -- the one the Work session
    //   JUST finished. Without this, a fresh-advance critique
    //   after milestone-N's Work would scope to milestone-(N+1)
    //   (the new "first pending") and the agent would critique
    //   the wrong milestone -- exactly the bug observed where
    //   DM3b's first critique reviewed an empty milestone-02
    //   instead of the milestone-01 work it should have evaluated.
    let milestone_scope: Option<String> = match step.milestone_walk {
        Some(walk) => {
            let pick_touched = match kind {
                SessionKind::Critique => true,
                SessionKind::Work => !prior_critique_gate_findings.is_empty(),
            };
            let resolved = match milestone_name {
                // Pinned worker (parallel plan-detail dispatcher).
                // Scope is the assigned stub regardless of walker
                // state -- a Critique that lands after its paired
                // Work cleared the placeholder still needs to read
                // the same file.
                Some(name) => {
                    crate::__internal::steps::find_milestone_by_name(project_dir, &walk, name)
                }
                None => crate::__internal::steps::find_current_milestone(
                    project_dir,
                    &walk,
                    pick_touched,
                ),
            };
            match resolved {
                crate::__internal::steps::CurrentMilestone::File(rel) => Some(rel),
                // AllResolved / NoMilestonesPresent: don't inject a
                // milestone scope. The structural gate
                // (MilestonesAllResolved) decides whether the step
                // can advance.
                _ => None,
            }
        }
        None => None,
    };

    // Two TOC buckets so volatile entries (the current milestone file)
    // can be emitted in a separate System message after the stable
    // ones, lengthening the prefix vLLM's KV cache can reuse across
    // milestone advances and critique retries within a step.
    let mut stable_toc: Vec<TocEntry> = Vec::new();
    let mut volatile_toc: Vec<TocEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push_stable = |rel: &str, seen: &mut std::collections::HashSet<String>| {
        if seen.insert(rel.to_string()) {
            stable_toc.push(toc_entry_for(project_dir, rel));
        }
    };
    if let Some(walk) = step.milestone_walk {
        // Keep predecessor inputs / work_artifacts that are NOT
        // inside the milestone directory (e.g. docs/spec.md,
        // docs/testbench.md, src/, tests/), drop everything that
        // points at or into walk.dir, then explicitly add the
        // plan index in the stable bucket. The current milestone
        // file goes into the volatile bucket below. The agent never
        // sees other milestone files exist (beyond any TOC inside
        // the index).
        let walk_dir = walk.dir.trim_end_matches('/');
        let inside_walk = |rel: &str| {
            let r = rel.trim_end_matches('/');
            r == walk_dir || r.starts_with(&format!("{walk_dir}/"))
        };
        for rel in step.predecessor_inputs {
            if !inside_walk(rel) {
                push_stable(rel, &mut seen);
            }
        }
        for rel in step.work_artifacts {
            if !inside_walk(rel) {
                push_stable(rel, &mut seen);
            }
        }
        push_stable(walk.index_file, &mut seen);
    } else {
        for rel in step.predecessor_inputs {
            push_stable(rel, &mut seen);
        }
        for rel in step.work_artifacts {
            push_stable(rel, &mut seen);
        }
    }
    if let Some(milestone_rel) = &milestone_scope
        && seen.insert(milestone_rel.clone())
    {
        volatile_toc.push(toc_entry_for(project_dir, milestone_rel));
    }
    // Only TOC the critique file when its body exists. The OLD
    // build always added a "(not yet on disk)" entry for the
    // critique file even on a fresh Work session, which both
    // misled the Work agent into thinking it should write the
    // critique file AND added a synthetic volatile message that
    // broke prefix caching across the very first dispatch.
    if inline_critique && critique_body.is_some() && seen.insert(critique_rel.clone()) {
        volatile_toc.push(toc_entry_for(project_dir, &critique_rel));
    }

    if stable_toc.is_empty() && volatile_toc.is_empty() && !inline_critique {
        return None;
    }

    let mut stable = format!(
        "Step `{}` inputs and target artifacts. File entries show path + size; \
         directory entries are expanded one level so you can see what's actually \
         on disk WITHOUT calling `list_dir`. Use `read_file` to fetch file content \
         on demand; do NOT assume a file's content is inlined here, and do NOT \
         claim a directory is empty unless its expansion below is empty.\n\n",
        step.id
    );
    for entry in &stable_toc {
        stable.push_str(&entry.render_block(project_dir));
    }

    let mut volatile = String::new();
    // Milestone-scope preamble. The agent's prompt already mentions
    // milestone walking, but the orchestrator-injected preamble
    // makes the CURRENT milestone unambiguous and tells the agent
    // not to read or write any sibling milestone file -- the
    // structural enforcement that the prompt-only "STOP after each
    // milestone" instruction failed to deliver in earlier runs.
    if let (Some(walk), Some(milestone_rel)) = (step.milestone_walk, &milestone_scope) {
        let session_label = match kind {
            SessionKind::Work => "work",
            SessionKind::Critique => "critique",
        };
        // Resolution criterion + sibling-protection wording differ
        // between execution-mode walks (DM2d / DM3b / DM3c / DM4b)
        // and planning-detail walks (DM2cd / DM3ad / DM4ad). The
        // execution-mode prompt talks about `- [ ]` rows resolving;
        // the detail-mode prompt talks about replacing the
        // outline's stub with a full task list.
        let prefix_pattern = walk.file_prefixes.to_vec().join("` / `");
        let resolution_clause = if let Some(marker) = walk.placeholder_marker {
            format!(
                "When the placeholder marker (`{marker}`) is gone from the current \
                 milestone -- meaning you've replaced the stub with a full task \
                 list per the format specified by your prompt"
            )
        } else if walk.forbid_deferred {
            "When EVERY row in the current milestone is `- [x]` done. \
             This step does NOT permit `- [-]` deferrals: they cannot \
             persist past this gate. If a row in this milestone is \
             currently `- [-]`, you must re-open it (set it back to \
             `- [ ]`), implement it, then mark it `- [x]`. Defers are \
             allowed during the work session for intra-step ordering, \
             but every one must land as `- [x]` before you signal the \
             milestone complete"
                .to_string()
        } else {
            "When all `- [ ]` rows in the current milestone are resolved \
             (`- [x]` done OR `- [-]` deferred with a `defer reason:` sub-bullet)"
                .to_string()
        };
        volatile.push_str(&format!(
            "**Milestone scope (orchestrator-enforced)**: this {session_label} \
             session targets EXACTLY ONE milestone -- `{milestone_rel}`. The plan \
             index `{}` is listed in the TOC above; read it with `read_file` \
             when you need its scope blurbs. You MUST NOT read or modify any \
             other `{prefix_pattern}<NN>-*.md` file in this session; sibling \
             milestones are intentionally hidden so each gets its own focused \
             critique. {resolution_clause}, stop and surface the canonical \
             `<milestone-name> complete; ready for critique.` notice. The \
             auto-driver will run the paired critique, then re-launch a \
             fresh session for the next milestone.\n\n",
            walk.index_file,
        ));
    }
    for entry in &volatile_toc {
        volatile.push_str(&entry.render_block(project_dir));
    }

    if inline_critique && (critique_body.is_some() || is_critique_retry) {
        volatile.push_str("\n---\n\n");
        if is_critique_retry {
            // `prior_critique_gate_findings` was already JSON-first
            // resolved at the top of the function; reuse it so the
            // inline blocks match the count the gate / auto driver
            // see.
            let blocks = &prior_critique_gate_findings;
            volatile.push_str(&format!(
                "Critique-retry mode. The prior pass flagged the gate-failing findings below; \
                 the work session has since re-run. Your task on THIS pass is FOCUSED:\n\n\
                 - For each prior finding, decide whether the work session's updated \
                   artifact resolves it. Quote the gap from the prior block if it is \
                   still applicable so the resolution is traceable.\n\
                 - Write the new critique fresh: emit `RESOLVED:` / `BLOCKER:` / \
                   `UNRESOLVED:` lines for the items below. Do NOT carry over the \
                   prior pass's RESOLVED / UNRESOLVED items verbatim -- those have \
                   been intentionally OMITTED from this context to keep your scope \
                   tight. They were closed in the prior pass; only re-flag if the \
                   new work introduced a regression.\n\
                 - Do NOT re-derive the full structural evaluation. Do NOT raise NEW \
                   `BLOCKER:` items unless the work session introduced a fresh \
                   problem (e.g. broke a previously-clean section). New `UNRESOLVED:` \
                   items surfaced by this turn's changes are fine.\n\n\
                 Prior BLOCKER(s) ({}) to re-evaluate:\n\n",
                blocks.len(),
            ));
            const RETRY_BLOCK_CAP: usize = 4_000;
            for (i, block) in blocks.iter().enumerate() {
                volatile.push_str(&format!(
                    "#### Prior BLOCKER {} of {}\n\n",
                    i + 1,
                    blocks.len()
                ));
                if block.len() <= RETRY_BLOCK_CAP {
                    volatile.push_str(block);
                } else {
                    // Surface truncation explicitly so the agent
                    // doesn't silently fix the wrong part of the
                    // BLOCKER, and log a metric so the cap can be
                    // raised if it bites recurrently.
                    tracing::warn!(
                        target: "sim_flow::metrics",
                        event = "critique_retry_block_truncated",
                        step = step.id,
                        block_index = i,
                        block_bytes = block.len(),
                        cap_bytes = RETRY_BLOCK_CAP,
                    );
                    volatile.push_str(&block[..RETRY_BLOCK_CAP]);
                    volatile.push_str(&format!(
                        "\n\n... [truncated to {RETRY_BLOCK_CAP} chars; original was {} chars. \
                         The full BLOCKER body is in the prior critique file -- \
                         re-read `{critique_rel}` if you need the tail.]",
                        block.len(),
                    ));
                }
                volatile.push_str("\n\n");
            }
        } else if let Some(body) = &critique_body {
            volatile.push_str(
                "Latest critique for this step (inlined because addressing these findings is your immediate task):\n\n",
            );
            volatile.push_str(&format!(
                "### `{critique_rel}`\n\n{}",
                truncate(body, 16_000),
            ));
        }
    }

    // Inline the orchestrator's most recent post-Work cargo report
    // (fmt-check + clippy) into the Critique session input. Lives at
    // `.sim-flow/cargo-checks-{step}.md` and gets overwritten each
    // milestone advance; the Critique now sees authoritative cargo
    // state instead of guessing from the Work transcript. Skip on
    // Work sessions -- Work writes the code, then the orchestrator
    // runs the checks AFTER, so Work has nothing fresh to read.
    if kind == SessionKind::Critique {
        let cargo_report_rel = format!(".sim-flow/cargo-checks-{}.md", step.id);
        let cargo_report_abs = project_dir.join(&cargo_report_rel);
        if let Ok(report_body) = std::fs::read_to_string(&cargo_report_abs) {
            volatile.push_str("\n---\n\n");
            volatile.push_str(&report_body);
        }
    }

    let volatile = if volatile.is_empty() {
        None
    } else {
        Some(volatile)
    };
    Some(SessionInputs { stable, volatile })
}

struct TocEntry {
    rel: String,
    state: TocState,
}

enum TocState {
    Directory,
    File {
        bytes: u64,
    },
    /// Small file whose contents are inlined directly into the
    /// session inputs message so the agent doesn't have to spend a
    /// `read_file` tool turn fetching it. Used for predecessor
    /// inputs whose body fits under
    /// `SIM_FLOW_INLINE_INPUT_THRESHOLD_BYTES` (default 4096).
    /// Eliminates 5-10 turns of overhead per Critique on a typical
    /// step that reads spec.md / decomposition.md / data-movement.md
    /// / etc.
    InlinedFile {
        bytes: u64,
        body: String,
    },
    Missing,
}

impl TocEntry {
    /// Render this entry as one or more bullet lines. Directories
    /// expand one level deep so the model can SEE the file list and
    /// can't hallucinate "empty"; nested directories are still
    /// summarized as `(directory, N entries)` so the prompt doesn't
    /// recurse without bound. Small files are inlined as fenced code
    /// blocks so the agent can read them without a tool turn.
    fn render_block(&self, project_dir: &Path) -> String {
        match &self.state {
            TocState::Directory => render_directory_block(project_dir, &self.rel),
            TocState::File { bytes } => format!("- `{}` ({} bytes)\n", self.rel, bytes),
            TocState::InlinedFile { bytes, body } => {
                let lang = inline_lang_hint(&self.rel);
                format!(
                    "- `{}` ({} bytes, inlined below):\n\n```{}\n{}\n```\n\n",
                    self.rel,
                    bytes,
                    lang,
                    body.trim_end()
                )
            }
            TocState::Missing => format!("- `{}` (not yet on disk)\n", self.rel),
        }
    }
}

/// Pick a fenced-block language hint from a path. Markdown stays
/// markdown so nested fences don't break the agent's parser; Rust
/// gets `rust`; everything else falls back to a generic `text`
/// fence which is safe for arbitrary content.
fn inline_lang_hint(rel: &str) -> &'static str {
    match std::path::Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some(ext) if ext.eq_ignore_ascii_case("md") => "markdown",
        Some(ext) if ext.eq_ignore_ascii_case("rs") => "rust",
        Some(ext) if ext.eq_ignore_ascii_case("toml") => "toml",
        Some(ext) if ext.eq_ignore_ascii_case("json") => "json",
        _ => "text",
    }
}

/// Per-file threshold below which `toc_entry_for` inlines the body.
/// Default is 0 (inlining disabled) -- the rule of the flow is
/// "every spec / plan / analysis doc is paginated or single-file
/// referenced via TOC; the agent reads what it needs with
/// `read_file`." Inlining small files saved 5-10 tool turns per
/// Critique on tiny projects but broke the principle for them
/// (large projects always hit the threshold and behaved correctly).
/// Trading a few extra tool turns on small projects for a uniform
/// "everything is read on demand" contract.
///
/// The inlining machinery is kept in place behind the env var
/// `SIM_FLOW_INLINE_INPUT_THRESHOLD_BYTES` so we can re-enable it
/// (set to e.g. 4096) without code changes if we change our mind.
fn inline_input_threshold_bytes() -> u64 {
    std::env::var("SIM_FLOW_INLINE_INPUT_THRESHOLD_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

fn render_directory_block(project_dir: &Path, rel: &str) -> String {
    let abs = project_dir.join(rel);
    let entries = match std::fs::read_dir(&abs) {
        Ok(it) => it.filter_map(|e| e.ok()).collect::<Vec<_>>(),
        Err(_) => {
            return format!("- `{rel}` (directory; could not be read)\n");
        }
    };
    if entries.is_empty() {
        return format!("- `{rel}` (directory, EMPTY)\n");
    }
    let mut listings: Vec<(String, String)> = Vec::with_capacity(entries.len());
    for ent in entries {
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // hide dotfiles (.gitkeep, .DS_Store, etc.)
        }
        let suffix = match ent.file_type() {
            Ok(ft) if ft.is_dir() => {
                let n = std::fs::read_dir(ent.path())
                    .map(|it| {
                        it.filter_map(|e| e.ok())
                            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                            .count()
                    })
                    .unwrap_or(0);
                format!(
                    "/ (directory, {n} entr{})",
                    if n == 1 { "y" } else { "ies" }
                )
            }
            Ok(_) => match ent.metadata() {
                Ok(m) => format!(" ({} bytes)", m.len()),
                Err(_) => String::from(" (size unknown)"),
            },
            Err(_) => String::new(),
        };
        listings.push((name.clone(), suffix));
    }
    if listings.is_empty() {
        return format!("- `{rel}` (directory, EMPTY)\n");
    }
    listings.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = format!(
        "- `{rel}` (directory, {} entr{}):\n",
        listings.len(),
        if listings.len() == 1 { "y" } else { "ies" }
    );
    for (name, suffix) in listings {
        out.push_str(&format!("  - {name}{suffix}\n"));
    }
    out
}

fn toc_entry_for(project_dir: &Path, rel: &str) -> TocEntry {
    if rel.ends_with('/') {
        return TocEntry {
            rel: rel.to_string(),
            state: TocState::Directory,
        };
    }
    let abs = project_dir.join(rel);
    match std::fs::metadata(&abs) {
        Ok(meta) if meta.is_dir() => TocEntry {
            rel: rel.to_string(),
            state: TocState::Directory,
        },
        Ok(meta) => {
            let bytes = meta.len();
            let threshold = inline_input_threshold_bytes();
            // Try to inline small text-shaped files. Binary files
            // (.png, .jpg, .pdf, .db) stay as plain TOC entries
            // even when small -- they aren't useful to the agent
            // as fenced text and would corrupt the markdown.
            if threshold > 0
                && bytes <= threshold
                && is_inlinable_extension(rel)
                && let Ok(body) = std::fs::read_to_string(&abs)
            {
                return TocEntry {
                    rel: rel.to_string(),
                    state: TocState::InlinedFile { bytes, body },
                };
            }
            TocEntry {
                rel: rel.to_string(),
                state: TocState::File { bytes },
            }
        }
        Err(_) => TocEntry {
            rel: rel.to_string(),
            state: TocState::Missing,
        },
    }
}

/// Whitelist of extensions safe to inline as fenced text. Markdown,
/// Rust source, TOML configs, JSON (e.g. critique.json), shell,
/// plain text, and the docs we know are always small. Skip
/// binaries and large generated artifacts even if they happen to
/// fall under the byte threshold.
fn is_inlinable_extension(rel: &str) -> bool {
    let ext = std::path::Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "rs" | "toml" | "json" | "txt" | "sh" | "yml" | "yaml" | ""
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\n... (truncated)", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_lang_hint_maps_known_extensions_case_insensitively() {
        assert_eq!(inline_lang_hint("docs/spec.md"), "markdown");
        assert_eq!(inline_lang_hint("src/lib.rs"), "rust");
        assert_eq!(inline_lang_hint("Cargo.toml"), "toml");
        assert_eq!(inline_lang_hint("data.json"), "json");
        // Uppercase extensions round-trip too.
        assert_eq!(inline_lang_hint("README.MD"), "markdown");
        // Unknown extension -> "text" fallback.
        assert_eq!(inline_lang_hint("script.py"), "text");
        // No extension -> "text".
        assert_eq!(inline_lang_hint("Makefile"), "text");
    }

    #[test]
    fn is_inlinable_extension_accepts_text_shaped_files_only() {
        for rel in [
            "x.md", "x.rs", "x.toml", "x.json", "x.txt", "x.sh", "x.yml", "x.yaml", "no-ext",
        ] {
            assert!(is_inlinable_extension(rel), "{rel}");
        }
        for rel in ["x.png", "x.jpg", "x.pdf", "x.db", "x.so", "x.bin"] {
            assert!(!is_inlinable_extension(rel), "{rel}");
        }
    }

    #[test]
    fn truncate_passes_through_under_cap_and_appends_marker_over() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello\n... (truncated)");
        // Exactly cap-length: under-equal branch keeps the whole body.
        assert_eq!(truncate("abcde", 5), "abcde");
    }
}
