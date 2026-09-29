# Controlled Live-Session Generation

Use this procedure when a provider capability must be verified but the existing local corpus has no representative artifact. Until generation or writer evidence resolves it, label the capability writer-supported but unobserved or unknown—not unsupported. The goal is a real source-format sample produced by the target tool, with synthetic content and tightly bounded effects. Do not generate sessions merely to replace adequate existing evidence.

## Authority and preflight

Running an agent can contact a remote service, consume credits, write local history, and execute tools. Do it only when the user requested live provider validation or otherwise authorized those effects. Before invoking it:

- inspect the installed version and `--help` instead of assuming flags;
- confirm authentication/model availability without printing identity, tokens, or account details;
- determine whether non-interactive mode persists the same session format;
- snapshot the provider source tree structurally so newly written transcripts and sidecars can be identified exactly;
- use a disposable empty working directory unless the behavior specifically requires a repository;
- use synthetic prompts, names, paths, and expected outputs with no user content or secrets.

If authentication, credits, a required model, or a safe execution mode is unavailable, report the blocked coverage rather than fabricating evidence or weakening safeguards.

## Prefer bounded non-interactive execution

Use the provider's print/headless mode when it writes normal persistent history. Select only flags confirmed by its current help/source. Where supported:

- cap turns to the minimum that can complete the feature;
- enable only the required tool or capability;
- choose plan/read-only permissions for inspection-only prompts;
- disable onboarding and automatic updates;
- request machine-readable output for diagnostics, but do not copy model text into fixtures or reports;
- do not use an in-memory/no-session flag, because the persisted artifact is the evidence;
- never use unrestricted/yolo permissions for a parser fixture.

Design one capability per run. For a subagent sample, start with one small foreground delegated task and a deterministic harmless final answer. If the tool supports background execution, generate a separate bounded sample that launches and explicitly waits for the result. If it supports parallel fan-out, generate a separate sample with at least two synthetically distinct delegated tasks in one parent turn and prove that every call id pairs with the correct result. A successful parent response is insufficient: inspect the transcript and prove the typed tool call, paired result, child id or inline scope, lifecycle, and usage that the provider actually persisted. Record absent fields too; absence of a child transcript or lifecycle record limits the normalized child but does not negate a typed delegated call.

## Use tmux only for TTY-required behavior

Interactive flows such as slash commands, branch selection, rewind, resume, or terminal-only image attachment may require a real TTY. In that case:

1. create a uniquely named detached tmux session in the disposable working directory;
2. send exact literal keystrokes rather than interpolating session content into a shell command;
3. use bounded waits and periodically capture the pane to distinguish progress, prompts, completion, and failure;
4. answer only expected permission prompts within the authorized scope;
5. exit the target cleanly, then terminate the exact tmux session if it remains;
6. verify the files written on disk, not just the captured terminal output.

Do not use tmux when a persistent non-interactive mode proves the same behavior more safely and deterministically.

## Artifact and coverage audit

Compare the source tree before and after each run. Identify only the new or changed transcript, metadata, child logs, assets, indexes, and WAL/sidecars. Inspect structural keys and typed relationships without echoing prompts, model output, ids, or image payloads into normal logs.

For each generated capability, establish the full evidence chain:

- subagent: spawn call → typed call/runtime id → child or inline records → completion/result → usage ownership;
- branching/rewind: selected leaf → exact parent chain → abandoned calls retained or excluded as the provider defines;
- image: visible content block → MIME/source or asset id → resolvable stored bytes;
- tools: call id → result id, including failure/incomplete behavior;
- usage: model call → timestamp/model → disjoint token fields and provider cost where present.

Add only synthetic equivalents to committed fixtures. Keep a generated real session until parser, isolated index, headless/UI, and resume checks are complete. If cleanup is requested or appropriate, remove only the exact artifacts created by the run using a recoverable operation, and report what was removed. Never bulk-delete the provider history root.
