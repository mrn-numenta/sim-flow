//! Argument-parsing tests for the sim-flow CLI. Each test parses a
//! representative argv vector through clap and matches the resulting
//! `Cli` against the expected `Command` shape. Catches regressions
//! when flag names / defaults / subcommand wiring drift.

use super::*;

fn parse(argv: &[&str]) -> Cli {
    Cli::try_parse_from(argv).expect("clap should accept the test argv")
}

// ---------- StepMode / FlowArg conversions ----------

#[test]
fn step_mode_into_protocol_round_trips() {
    assert_eq!(
        sim_flow::__internal::session::protocol::StepMode::from(StepMode::Auto),
        sim_flow::__internal::session::protocol::StepMode::Auto,
    );
    assert_eq!(
        sim_flow::__internal::session::protocol::StepMode::from(StepMode::Manual),
        sim_flow::__internal::session::protocol::StepMode::Manual,
    );
}

#[test]
fn flow_arg_into_state_flow_round_trips() {
    assert!(matches!(
        Flow::from(FlowArg::DirectModeling),
        Flow::DirectModeling
    ));
    assert!(matches!(
        Flow::from(FlowArg::DesignStudy),
        Flow::DesignStudy
    ));
    assert!(matches!(
        Flow::from(FlowArg::SystemverilogConvert),
        Flow::SystemVerilogConvert
    ));
}

#[test]
fn convert_sv_parses_without_force() {
    let cli = parse(&["sim-flow", "convert-sv"]);
    match cli.command {
        Command::ConvertSv { force } => assert!(!force, "default --force should be false"),
        other => panic!("expected Command::ConvertSv, got {other:?}"),
    }
}

#[test]
fn convert_sv_parses_with_force() {
    let cli = parse(&["sim-flow", "convert-sv", "--force"]);
    match cli.command {
        Command::ConvertSv { force } => assert!(force, "--force must propagate"),
        other => panic!("expected Command::ConvertSv, got {other:?}"),
    }
}

#[test]
fn bugs_list_parses_with_filters() {
    let cli = parse(&[
        "sim-flow",
        "bugs",
        "list",
        "--open",
        "--step",
        "DM3c",
        "--category",
        "framework",
    ]);
    match cli.command {
        Command::Bugs {
            action:
                BugsAction::List {
                    open,
                    resolved,
                    step,
                    category,
                },
        } => {
            assert!(open);
            assert!(!resolved);
            assert_eq!(step.as_deref(), Some("DM3c"));
            assert_eq!(category.as_deref(), Some("framework"));
        }
        other => panic!("expected Command::Bugs(List), got {other:?}"),
    }
}

#[test]
fn bugs_show_requires_id() {
    let cli = parse(&["sim-flow", "bugs", "show", "bug-001"]);
    match cli.command {
        Command::Bugs {
            action: BugsAction::Show { id },
        } => assert_eq!(id, "bug-001"),
        other => panic!("expected Command::Bugs(Show), got {other:?}"),
    }
}

#[test]
fn init_accepts_systemverilog_convert_flow() {
    let cli = parse(&["sim-flow", "init", "--flow", "systemverilog-convert"]);
    match cli.command {
        Command::Init { flow } => {
            let f: Flow = flow.into();
            assert!(matches!(f, Flow::SystemVerilogConvert));
        }
        other => panic!("expected Command::Init, got {other:?}"),
    }
}

// ---------- Auto subcommand: --llm-base-url plumbing ----------

#[test]
fn auto_default_omits_llm_base_url() {
    let cli = parse(&["sim-flow", "auto"]);
    match cli.command {
        Command::Auto {
            llm_base_url,
            llm_backend,
            ..
        } => {
            assert_eq!(llm_base_url, None);
            assert_eq!(llm_backend, "vscode");
        }
        other => panic!("expected Command::Auto, got {other:?}"),
    }
}

#[test]
fn auto_accepts_llm_base_url_flag() {
    let cli = parse(&[
        "sim-flow",
        "auto",
        "--llm-base-url",
        "http://my-vllm:8000/v1",
    ]);
    match cli.command {
        Command::Auto { llm_base_url, .. } => {
            assert_eq!(llm_base_url.as_deref(), Some("http://my-vllm:8000/v1"));
        }
        other => panic!("expected Command::Auto, got {other:?}"),
    }
}

#[test]
fn auto_accepts_llm_base_url_with_other_llm_flags() {
    let cli = parse(&[
        "sim-flow",
        "auto",
        "--llm-backend",
        "vllm",
        "--llm-model",
        "qwen3.6:32b",
        "--llm-base-url",
        "http://prod-vllm:8000/v1",
    ]);
    match cli.command {
        Command::Auto {
            llm_backend,
            llm_model,
            llm_base_url,
            ..
        } => {
            assert_eq!(llm_backend, "vllm");
            assert_eq!(llm_model.as_deref(), Some("qwen3.6:32b"));
            assert_eq!(llm_base_url.as_deref(), Some("http://prod-vllm:8000/v1"));
        }
        other => panic!("expected Command::Auto, got {other:?}"),
    }
}

// ---------- Session subcommand: --llm-base-url plumbing ----------

#[test]
fn session_default_omits_all_base_url_flags() {
    let cli = parse(&["sim-flow", "session", "DM0.work"]);
    match cli.command {
        Command::Session {
            step_kind,
            ollama_base_url,
            openai_base_url,
            llm_base_url,
            ..
        } => {
            assert_eq!(step_kind, "DM0.work");
            assert_eq!(ollama_base_url, None);
            assert_eq!(openai_base_url, None);
            assert_eq!(llm_base_url, None);
        }
        other => panic!("expected Command::Session, got {other:?}"),
    }
}

#[test]
fn session_accepts_all_three_url_flags_independently() {
    // Setting all three is unusual but legal -- the precedence
    // resolution happens later in `commands.rs::session_cmd` and
    // is exercised by the agent-side `resolved_base_url` tests.
    let cli = parse(&[
        "sim-flow",
        "session",
        "DM2c.critique",
        "--llm-backend",
        "vllm",
        "--llm-base-url",
        "http://generic",
        "--ollama-base-url",
        "http://o:11434/v1",
        "--openai-base-url",
        "http://lm:1234/v1",
    ]);
    match cli.command {
        Command::Session {
            ollama_base_url,
            openai_base_url,
            llm_base_url,
            llm_backend,
            ..
        } => {
            assert_eq!(llm_backend, "vllm");
            assert_eq!(llm_base_url.as_deref(), Some("http://generic"));
            assert_eq!(ollama_base_url.as_deref(), Some("http://o:11434/v1"));
            assert_eq!(openai_base_url.as_deref(), Some("http://lm:1234/v1"));
        }
        other => panic!("expected Command::Session, got {other:?}"),
    }
}

#[test]
fn auto_default_session_mode_is_per_step() {
    let cli = parse(&["sim-flow", "auto"]);
    match cli.command {
        Command::Auto {
            session_mode,
            step_mode,
            no_preamble,
            ..
        } => {
            assert_eq!(session_mode, SessionMode::PerStep);
            assert_eq!(step_mode, StepMode::Auto);
            assert!(no_preamble);
        }
        other => panic!("expected Command::Auto, got {other:?}"),
    }
}

// ---------- Auto subcommand: --critique-llm-* plumbing ----------

#[test]
fn auto_default_omits_all_critique_llm_flags() {
    // Default `sim-flow auto` keeps the critique stack
    // implicit (everything falls back to the work-side
    // `--llm-*` knobs in the orchestrator's
    // `resolve_llm_for_kind`).
    let cli = parse(&["sim-flow", "auto"]);
    match cli.command {
        Command::Auto {
            critique_llm_backend,
            critique_llm_model,
            critique_llm_model_family,
            critique_llm_runtime_profile,
            critique_llm_base_url,
            ..
        } => {
            assert_eq!(critique_llm_backend, None);
            assert_eq!(critique_llm_model, None);
            assert_eq!(critique_llm_model_family, None);
            assert_eq!(critique_llm_runtime_profile, None);
            assert_eq!(critique_llm_base_url, None);
        }
        other => panic!("expected Command::Auto, got {other:?}"),
    }
}

#[test]
fn auto_accepts_critique_llm_backend_flag() {
    // The canonical use case: vLLM for work, Anthropic for
    // critique. Every flag should land in its own field; we
    // assert each one individually so a renamed destination
    // surfaces the regression.
    let cli = parse(&[
        "sim-flow",
        "auto",
        "--llm-backend",
        "vllm",
        "--llm-model",
        "qwen3.6",
        "--critique-llm-backend",
        "anthropic",
        "--critique-llm-model",
        "claude-3-5-sonnet-latest",
        "--critique-llm-model-family",
        "claude_messages",
        "--critique-llm-runtime-profile",
        "anthropic_messages",
        "--critique-llm-base-url",
        "https://api.anthropic.com",
    ]);
    match cli.command {
        Command::Auto {
            llm_backend,
            llm_model,
            critique_llm_backend,
            critique_llm_model,
            critique_llm_model_family,
            critique_llm_runtime_profile,
            critique_llm_base_url,
            ..
        } => {
            assert_eq!(llm_backend, "vllm");
            assert_eq!(llm_model.as_deref(), Some("qwen3.6"));
            assert_eq!(critique_llm_backend.as_deref(), Some("anthropic"));
            assert_eq!(
                critique_llm_model.as_deref(),
                Some("claude-3-5-sonnet-latest")
            );
            assert_eq!(
                critique_llm_model_family.as_deref(),
                Some("claude_messages")
            );
            assert_eq!(
                critique_llm_runtime_profile.as_deref(),
                Some("anthropic_messages")
            );
            assert_eq!(
                critique_llm_base_url.as_deref(),
                Some("https://api.anthropic.com")
            );
        }
        other => panic!("expected Command::Auto, got {other:?}"),
    }
}

#[test]
fn auto_partial_critique_override_parses_without_complaint() {
    // The CLI doesn't validate that critique flags are
    // self-consistent (e.g. model set but backend unset).
    // The orchestrator emits a Diagnostic at session start
    // when it spots a partial override (see auto.rs); this
    // test pins down that the CLI itself stays permissive
    // -- the warning is informational, not a parse error.
    let cli = parse(&[
        "sim-flow",
        "auto",
        "--critique-llm-model",
        "claude-3-5-sonnet-latest",
    ]);
    match cli.command {
        Command::Auto {
            critique_llm_backend,
            critique_llm_model,
            ..
        } => {
            assert_eq!(critique_llm_backend, None);
            assert_eq!(
                critique_llm_model.as_deref(),
                Some("claude-3-5-sonnet-latest")
            );
        }
        other => panic!("expected Command::Auto, got {other:?}"),
    }
}
