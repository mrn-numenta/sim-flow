// Workspace-scoped file watcher over the three on-disk state sources
// the dashboard cares about: state.toml, critique markdown files, and
// experiments.db. Emits a single coarse `change` event each time
// anything under `.sim-flow/` changes; consumers decide what to
// re-read. We accept the coarser granularity because each read is cheap
// (TOML is small, critiques are small, the sqlite DB open is fast).
//
// This module imports vscode and therefore cannot run inside
// vitest. Keep the logic here trivially thin: all reading / parsing
// lives in the sibling modules, which have their own tests.

import * as vscode from "vscode";

export type StateChangeKind =
  | "state-toml"
  | "critiques"
  | "experiments-db"
  | "plan";

export interface StateChangeEvent {
  projectDir: string;
  kind: StateChangeKind;
  uri: vscode.Uri;
}

export interface SimFlowStateWatcher extends vscode.Disposable {
  onDidChange: vscode.Event<StateChangeEvent>;
}

/**
 * Create a watcher over a specific sim-flow project directory. Call
 * `.dispose()` when the project is no longer of interest (e.g. the
 * user closed the workspace).
 */
export function createStateWatcher(projectDir: string): SimFlowStateWatcher {
  const emitter = new vscode.EventEmitter<StateChangeEvent>();
  const disposables: vscode.Disposable[] = [emitter];
  const base = vscode.Uri.file(projectDir);

  const register = (pattern: string, kind: StateChangeKind) => {
    const rel = new vscode.RelativePattern(base, pattern);
    const watcher = vscode.workspace.createFileSystemWatcher(rel);
    const fire = (uri: vscode.Uri) => {
      emitter.fire({ projectDir, kind, uri });
    };
    disposables.push(
      watcher,
      watcher.onDidCreate(fire),
      watcher.onDidChange(fire),
      watcher.onDidDelete(fire),
    );
  };

  register(".sim-flow/state.toml", "state-toml");
  // Match both forms of critique artifact: `.json` is the canonical
  // structured form the orchestrator and gate read; `.md` is the
  // rendered view. Watching only `.md` meant a JSON-only update
  // (e.g. agent emits JSON via `write_file` and the markdown render
  // failed or hasn't landed yet) didn't refresh the dashboard.
  register("docs/critiques/*.json", "critiques");
  register("docs/critiques/*.md", "critiques");
  register(".sim-flow/experiments.db", "experiments-db");
  // Plan files drive the progress panel under the step buttons. Edits
  // by the agent (flipping `- [ ]` to `- [x]`) need to refresh the
  // dashboard so the milestone boxes + current-task line stay live.
  //
  // Each DM tier has its own plan directory now (DM2 -> impl-plan/,
  // DM3 -> test-plan/, DM4 -> perf-plan/). The shared
  // `plan-management.md` lives one level up under `docs/`. Watch all
  // four globs so an agent edit anywhere refreshes the dashboard.
  // (The legacy `docs/plan/*.md` location was retired when
  // plan-management.md was moved to `docs/`; existing projects had
  // their plan-management.md migrated as part of that change.)
  register("docs/impl-plan/*.md", "plan");
  register("docs/test-plan/*.md", "plan");
  register("docs/perf-plan/*.md", "plan");
  register("docs/plan-management.md", "plan");
  // The single-session control socket. The orchestrator binds this
  // ~1s after `sim-flow auto --session-mode single` starts; presence
  // of the file is the dashboard's primary "session is live" signal
  // for CLI backends (which don't register with AutoSessionManager).
  // Without watching it, the dashboard wouldn't refresh until the
  // next unrelated state.toml / critique / plan event, leaving the
  // Connect button visually stuck in "connecting" up to 5 seconds.
  register(".sim-flow/control.sock", "state-toml");

  return {
    onDidChange: emitter.event,
    dispose(): void {
      for (const d of disposables) {
        d.dispose();
      }
    },
  };
}
