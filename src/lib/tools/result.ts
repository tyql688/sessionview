import type { ToolMetadata } from "../types";
import { buildPatchLineDiff, buildStructuredPatchLineDiff } from "../diff";
import { shortenHomePath } from "../formatters";
import {
  type Line,
  type ToolDetail,
  firstString,
  maybeNumber,
  nestedRecord,
  pickField,
  structuredRecord,
  toolLine,
  valueToDisplayString,
} from "./types";

function patchFiles(structured: Record<string, unknown>): string[] {
  const files = new Set<string>();
  const pushFiles = (value: unknown) => {
    if (!Array.isArray(value)) return;
    for (const file of value) {
      if (typeof file === "string" && file.length > 0) {
        files.add(shortenHomePath(file));
      }
    }
  };

  const patch = nestedRecord(structured.patch);
  pushFiles(patch?.files);

  if (Array.isArray(structured.patches)) {
    for (const item of structured.patches) {
      pushFiles(nestedRecord(item)?.files);
    }
  }

  return [...files];
}

function nestedStatusText(value: unknown): string {
  const record = nestedRecord(value);
  if (!record) return "";
  for (const key of [
    "completed",
    "failed",
    "running",
    "pending",
    "interrupted",
  ]) {
    const text = firstString(record, [key]);
    if (text) return text;
  }
  return "";
}

function mcpResultSummary(structured: Record<string, unknown>): string {
  const result = nestedRecord(structured.result);
  if (!result) return "";

  const err = result.Err;
  if (typeof err === "string" && err.length > 0) return err;

  const ok = nestedRecord(result.Ok);
  const content = ok?.content;
  if (!Array.isArray(content)) return "";

  const parts: string[] = [];
  for (const item of content) {
    const record = nestedRecord(item);
    const text = firstString(record ?? {}, ["text"]);
    if (text) parts.push(text);
  }
  return parts.join("\n");
}

/**
 * Shared shape passed to each per-canonical-name formatter. A formatter may
 * return additional `Line[]` to append to the base lines, OR a complete
 * `ToolDetail` to short-circuit (used when a diff/patch must be attached). The
 * complete-ToolDetail formatters receive `baseLines` (already carrying the
 * status row) and `persistedOutputPath` so they can build the final object.
 */
interface FormatterContext {
  structured: Record<string, unknown>;
  metadata: ToolMetadata;
  baseLines: Line[];
  persistedOutputPath?: string;
}

type ResultFormatter = (ctx: FormatterContext) => Line[] | ToolDetail;

function formatBashResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  const cwd = firstString(structured, ["cwd"]);
  if (cwd) lines.push(toolLine("cwd", cwd));

  const source = firstString(structured, ["source"]);
  if (source) lines.push({ label: "source", value: source });

  const exitCode = maybeNumber(structured.exitCode ?? structured.exit_code);
  if (exitCode) lines.push({ label: "exit", value: exitCode });

  const duration = maybeNumber(
    structured.durationSeconds ?? structured.duration_seconds,
  );
  if (duration) lines.push({ label: "duration", value: `${duration}s` });

  const stdout = firstString(structured, ["stdout", "output"]);
  if (stdout) lines.push({ label: "stdout", value: stdout });
  if (typeof structured.stderr === "string" && structured.stderr.length > 0) {
    lines.push({ label: "stderr", value: structured.stderr });
  }
  return lines;
}

function formatFileEditResult({
  structured,
  baseLines,
  persistedOutputPath,
}: FormatterContext): Line[] | ToolDetail {
  const lines = [...baseLines];
  // Alias chain: Claude uses filePath, codex/antigravity use file_path.
  const file = pickField(structured, ["filePath", "file_path"]);
  if (file) lines.push(toolLine("file", file));

  const metadataRecord = nestedRecord(structured.metadata);
  const fileDiffRecord = nestedRecord(metadataRecord?.filediff);

  const patchFilesList = patchFiles(structured);
  if (patchFilesList.length > 0) {
    lines.push({ label: "files", value: patchFilesList.join("\n") });
  }

  const patchText =
    firstString(structured, ["diff"]) ||
    firstString(metadataRecord ?? {}, ["diff"]) ||
    firstString(fileDiffRecord ?? {}, ["patch"]);
  if (patchText) {
    return {
      lines,
      patchDiff: buildPatchLineDiff(patchText),
      persistedOutputPath,
    };
  }

  const structuredPatch = buildStructuredPatchLineDiff(
    structured.structuredPatch,
  );
  if (structuredPatch.length > 0) {
    return { lines, patchDiff: structuredPatch, persistedOutputPath };
  }

  const oldText = firstString(structured, ["oldString", "old_string"]);
  const newText = firstString(structured, ["newString", "new_string"]);
  if (oldText || newText) {
    return { lines, diff: { old: oldText, new: newText }, persistedOutputPath };
  }

  if (
    structured.type === "create" &&
    typeof structured.content === "string" &&
    structured.content.length > 0
  ) {
    return {
      lines,
      diff: { old: "", new: structured.content },
      persistedOutputPath,
    };
  }
  // No diff/patch: hand back the appended lines (file/files) so the default
  // assembly can finalize them.
  return lines.slice(baseLines.length);
}

function formatAgentResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  for (const [label, key] of [
    ["agent", "agentId"],
    ["type", "agentType"],
    ["tokens", "totalTokens"],
    ["tools", "totalToolUseCount"],
  ] as const) {
    const value =
      typeof structured[key] === "string"
        ? structured[key]
        : maybeNumber(structured[key]);
    if (value) lines.push({ label, value });
  }

  const nickname = firstString(structured, [
    "nickname",
    "new_agent_nickname",
    "receiver_agent_nickname",
  ]);
  if (nickname) lines.push({ label: "nickname", value: nickname });

  const role = firstString(structured, [
    "new_agent_role",
    "receiver_agent_role",
  ]);
  if (role) lines.push({ label: "role", value: role });

  const model = firstString(structured, ["model"]);
  if (model) lines.push({ label: "model", value: model });

  const reasoning = firstString(structured, ["reasoning_effort"]);
  if (reasoning) lines.push({ label: "reasoning", value: reasoning });

  const senderThread = firstString(structured, ["sender_thread_id"]);
  if (senderThread) lines.push({ label: "sender", value: senderThread });

  const newThread = firstString(structured, ["new_thread_id"]);
  if (newThread) lines.push({ label: "newThread", value: newThread });

  const receiverThread = firstString(structured, ["receiver_thread_id"]);
  if (receiverThread) lines.push({ label: "receiver", value: receiverThread });

  if (structured.timed_out === true) {
    lines.push({ label: "timedOut", value: "true" });
  }

  const statusSummary =
    nestedStatusText(structured.status) ||
    nestedStatusText(structured.previous_status);
  if (statusSummary) {
    lines.push({ label: "statusText", value: statusSummary });
  }

  const agentStatuses = Array.isArray(structured.agent_statuses)
    ? structured.agent_statuses.length
    : undefined;
  if (agentStatuses && agentStatuses > 0) {
    lines.push({ label: "agentStatuses", value: String(agentStatuses) });
  } else {
    const statuses = nestedRecord(structured.statuses);
    if (statuses) {
      lines.push({
        label: "agentStatuses",
        value: String(Object.keys(statuses).length),
      });
    }
  }
  return lines;
}

function formatToolSearchResult({ structured }: FormatterContext): Line[] {
  return [
    { label: "query", value: String(structured.query || "") },
    {
      label: "matches",
      value: Array.isArray(structured.matches)
        ? String(structured.matches.length)
        : String(structured.total_deferred_tools || ""),
    },
  ];
}

function formatTaskResult({ structured, metadata }: FormatterContext): Line[] {
  const lines: Line[] = [];
  const task = nestedRecord(structured.task);

  if (metadata.canonical_name === "TaskCreate") {
    const id = firstString(task ?? structured, ["id", "taskId", "task_id"]);
    const subject = firstString(task ?? structured, ["subject", "description"]);
    if (id) lines.push({ label: "task", value: id });
    if (subject) lines.push({ label: "subject", value: subject });
    return lines;
  }

  if (metadata.canonical_name === "TaskList") {
    if (Array.isArray(structured.tasks)) {
      lines.push({ label: "tasks", value: String(structured.tasks.length) });
      const preview = structured.tasks
        .map((item) => {
          const record = nestedRecord(item);
          if (!record) return "";
          return firstString(record, [
            "subject",
            "description",
            "task_id",
            "id",
          ]);
        })
        .filter(Boolean)
        .slice(0, 5)
        .join("\n");
      if (preview) lines.push({ label: "preview", value: preview });
    }
    return lines;
  }

  if (metadata.canonical_name === "TaskOutput") {
    for (const [label, key] of [
      ["retrieval", "retrieval_status"],
      ["task", "task_id"],
      ["status", "status"],
      ["type", "task_type"],
      ["description", "description"],
      ["output", "output"],
    ] as const) {
      const value = firstString(task ?? structured, [key]);
      if (value) lines.push({ label, value });
    }
    return lines;
  }

  for (const key of [
    "taskId",
    "task_id",
    "task_type",
    "status",
    "statusChange",
    "updatedFields",
    "command",
    "message",
    "success",
  ]) {
    if (structured[key] !== undefined) {
      lines.push({
        label: key,
        value: valueToDisplayString(structured[key]),
      });
    }
  }
  return lines;
}

function formatWebSearchResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  const query = firstString(structured, ["query"]);
  if (query) lines.push({ label: "query", value: query });
  const searchCount = maybeNumber(structured.searchCount);
  if (searchCount) lines.push({ label: "searches", value: searchCount });
  const duration = maybeNumber(structured.durationSeconds);
  if (duration) lines.push({ label: "duration", value: `${duration}s` });
  if (Array.isArray(structured.results)) {
    lines.push({ label: "results", value: String(structured.results.length) });
  }
  return lines;
}

function formatWebFetchResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  for (const key of ["url", "code", "codeText", "bytes", "durationMs"]) {
    if (structured[key] !== undefined) {
      lines.push({ label: key, value: String(structured[key]) });
    }
  }
  const result = firstString(structured, ["result"]);
  if (result) lines.push({ label: "result", value: result });
  return lines;
}

function formatImageGenerationResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  const savedPath = firstString(structured, ["savedPath", "saved_path"]);
  if (savedPath) lines.push(toolLine("savedPath", savedPath));
  const prompt = firstString(structured, ["revisedPrompt", "revised_prompt"]);
  if (prompt) lines.push({ label: "revisedPrompt", value: prompt });
  return lines;
}

function formatDynamicToolResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  const tool = firstString(structured, ["tool", "name"]);
  if (tool) lines.push({ label: "tool", value: tool });
  if (typeof structured.success === "boolean") {
    lines.push({
      label: "success",
      value: structured.success ? "true" : "false",
    });
  }
  const duration = nestedRecord(structured.duration);
  const secs = typeof duration?.secs === "number" ? duration.secs : undefined;
  const nanos =
    typeof duration?.nanos === "number" ? duration.nanos : undefined;
  if (secs !== undefined || nanos !== undefined) {
    lines.push({
      label: "duration",
      value: `${((secs ?? 0) + (nanos ?? 0) / 1_000_000_000).toLocaleString()}s`,
    });
  }
  const content = firstString(structured, ["content"]);
  if (content) lines.push({ label: "result", value: content });
  const error = firstString(structured, ["error"]);
  if (error) lines.push({ label: "error", value: error });
  return lines;
}

function formatQuestionResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  if (Array.isArray(structured.questions)) {
    lines.push({
      label: "questions",
      value: String(structured.questions.length),
    });
  }
  const answers = nestedRecord(structured.answers);
  if (answers && Object.keys(answers).length > 0) {
    lines.push({ label: "answers", value: valueToDisplayString(answers) });
  }
  return lines;
}

function formatScheduleResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  for (const key of ["scheduledFor", "clampedDelaySeconds", "wasClamped"]) {
    if (structured[key] !== undefined) {
      lines.push({ label: key, value: valueToDisplayString(structured[key]) });
    }
  }
  return lines;
}

function formatSkillResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  const command = firstString(structured, ["commandName", "skill"]);
  if (command) lines.push({ label: "command", value: command });
  if (typeof structured.success === "boolean") {
    lines.push({
      label: "success",
      value: structured.success ? "true" : "false",
    });
  }
  return lines;
}

function formatWorkflowResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  for (const key of [
    "workflowName",
    "status",
    "summary",
    "runId",
    "taskId",
    "taskType",
  ]) {
    if (structured[key] !== undefined) {
      lines.push({ label: key, value: valueToDisplayString(structured[key]) });
    }
  }
  for (const key of ["scriptPath", "transcriptDir"]) {
    const value = firstString(structured, [key]);
    if (value) lines.push(toolLine(key, value));
  }
  return lines;
}

function formatOutputResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  for (const key of [
    "output",
    "content",
    "result",
    "stdout",
    "stderr",
    "error",
  ]) {
    const value = firstString(structured, [key]);
    if (value) lines.push({ label: key, value });
  }
  if (typeof structured.success === "boolean") {
    lines.push({
      label: "success",
      value: structured.success ? "true" : "false",
    });
  }
  const duration = maybeNumber(
    structured.durationSeconds ?? structured.duration_seconds,
  );
  if (duration) lines.push({ label: "duration", value: `${duration}s` });
  return lines;
}

function formatGoalResult({ structured }: FormatterContext): Line[] {
  const lines: Line[] = [];
  for (const key of [
    "status",
    "objective",
    "remainingTokens",
    "token_budget",
    "completionBudgetReport",
  ]) {
    if (structured[key] !== undefined) {
      lines.push({ label: key, value: valueToDisplayString(structured[key]) });
    }
  }
  return lines;
}

function appendCallMetadataLines(
  lines: Line[],
  structured: Record<string, unknown>,
) {
  const description = firstString(structured, ["callDescription"]);
  if (description) lines.push({ label: "description", value: description });

  const display = nestedRecord(structured.callDisplay);
  if (!display) return;
  for (const key of [
    "kind",
    "operation",
    "path",
    "cwd",
    "language",
    "command",
    "agent_name",
  ]) {
    const value = display[key];
    if (typeof value === "string" && value.length > 0) {
      lines.push(toolLine(key, value));
    }
  }
}

/** Per-canonical-name result formatters. Names not present here fall through
 *  to the category-based default below. */
const RESULT_FORMATTERS: Record<string, ResultFormatter> = {
  Bash: formatBashResult,
  Edit: formatFileEditResult,
  Write: formatFileEditResult,
  Agent: formatAgentResult,
  TaskCreate: formatTaskResult,
  TaskUpdate: formatTaskResult,
  TaskList: formatTaskResult,
  TaskOutput: formatTaskResult,
  TaskStop: formatTaskResult,
  ToolSearch: formatToolSearchResult,
  WebSearch: formatWebSearchResult,
  WebFetch: formatWebFetchResult,
  ImageGeneration: formatImageGenerationResult,
  DynamicTool: formatDynamicToolResult,
  JavaScript: formatOutputResult,
  ComputerUse: formatOutputResult,
  StructuredOutput: formatOutputResult,
  SendMessage: formatOutputResult,
  AskUserQuestion: formatQuestionResult,
  ScheduleWakeup: formatScheduleResult,
  Skill: formatSkillResult,
  Workflow: formatWorkflowResult,
  ReadMediaFile: formatOutputResult,
  CreateGoal: formatGoalResult,
  GetGoal: formatGoalResult,
  SetGoalBudget: formatGoalResult,
  UpdateGoal: formatGoalResult,
};

/** Category-based fallback for canonical names without a dedicated formatter. */
function formatDefaultResult({
  structured,
  metadata,
}: FormatterContext): Line[] {
  const lines: Line[] = [];
  if (metadata.category === "task") {
    for (const key of ["taskId", "task_id", "statusChange", "message"]) {
      if (structured[key] !== undefined) {
        lines.push({
          label: key,
          value: valueToDisplayString(structured[key]),
        });
      }
    }
  } else if (metadata.category === "mcp" && metadata.mcp) {
    lines.push(
      { label: "server", value: metadata.mcp.server },
      { label: "tool", value: metadata.mcp.tool },
    );
    if (typeof structured.success === "boolean") {
      lines.push({
        label: "success",
        value: structured.success ? "true" : "false",
      });
    }
    const duration = maybeNumber(
      structured.durationSeconds ?? structured.duration_seconds,
    );
    if (duration) {
      lines.push({ label: "duration", value: `${duration}s` });
    }
    const invocation = nestedRecord(structured.invocation);
    const argumentsValue = invocation?.arguments;
    if (argumentsValue !== undefined) {
      lines.push({
        label: "args",
        value: valueToDisplayString(argumentsValue),
      });
    }
    const resultText = mcpResultSummary(structured);
    if (resultText) {
      lines.push({ label: "result", value: resultText });
    }
  }
  return lines;
}

export function formatToolResultMetadata(
  metadata?: ToolMetadata,
): ToolDetail | null {
  const structured = structuredRecord(metadata);
  if (!metadata || !structured) return null;

  const persistedOutputPath =
    typeof structured.persistedOutputPath === "string"
      ? structured.persistedOutputPath
      : undefined;

  const baseLines: Line[] = [];
  if (metadata.status) {
    baseLines.push({ label: "status", value: metadata.status });
  }
  appendCallMetadataLines(baseLines, structured);

  const ctx: FormatterContext = {
    structured,
    metadata,
    baseLines,
    persistedOutputPath,
  };
  const formatter = RESULT_FORMATTERS[metadata.canonical_name];
  const produced = formatter ? formatter(ctx) : formatDefaultResult(ctx);

  // A formatter returning a complete ToolDetail short-circuits (diff/patch
  // already attached); otherwise it returns extra lines to append.
  if (!Array.isArray(produced)) return produced;

  const lines = [...baseLines, ...produced];
  return lines.length > 0 || persistedOutputPath
    ? { lines, persistedOutputPath }
    : null;
}
