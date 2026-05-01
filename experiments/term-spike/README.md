# Embedded-terminal emulator spike

Three minimal egui apps, each embedding one terminal-emulation backend so
we can compare fidelity on real AI CLIs (Claude Code, Codex, Copilot)
before picking one for `sim-flow-gui`.

All three share the same egui window, PTY plumbing, cell-grid renderer,
and keystroke forwarding via the [`term-core`](term-core/) crate. The
only difference between demos is the backend behind the `TermBackend`
trait.

## Demos

| Demo | Backend | Crate |
| ---- | ------- | ----- |
| `demo-alacritty` | Alacritty's VT engine | `alacritty_terminal` |
| `demo-wezterm`   | WezTerm's terminal   | `wezterm-term` |
| `demo-rio`       | Rio's VT engine      | `copa` + `rio-backend` |

## Running

```bash
cd experiments/term-spike

# Default: each demo spawns $SHELL so you have something to type at.
cargo run -p demo-alacritty
cargo run -p demo-wezterm
cargo run -p demo-rio

# Spawn a specific command (e.g. claude) by passing it as args:
cargo run -p demo-alacritty -- claude
cargo run -p demo-alacritty -- codex
cargo run -p demo-alacritty -- vim
```

## What we're evaluating

For each backend, we want to know:

1. Does Claude Code's TUI render correctly? Alt-screen switching, the
   message pane, the input prompt area, cursor position.
2. Colors (256-color, true color) come through intact.
3. Unicode and box-drawing characters render.
4. Typing feels responsive; Enter submits, arrow keys navigate, Ctrl-C
   interrupts.
5. Resizing the window propagates to the PTY and the child re-renders.

## Scope

This is a spike, not production code. It intentionally omits:

- Scrollback beyond the visible grid.
- Mouse passthrough to the TUI.
- Bell, hyperlinks (OSC 8), bracketed paste edge cases.
- Sixel or inline image support.
- Per-cell styling beyond fg/bg/bold (italic, underline, etc).

## Workspace isolation

This workspace is declared as excluded from the top-level
`sim-foundation` workspace (see `[workspace.exclude]` in the root
`Cargo.toml`) so the heavy / unstable terminal deps do not leak into
the main build graph.
