import type { Message } from "../types";
import { buildPatchLineDiff } from "../diff";
import { shortenHomePath } from "../formatters";
import { type Line, type ToolDetail, toolLine } from "./types";

function extractPatchedFiles(patchText: string): string[] {
  const files = patchText
    .split("\n")
    .map((line) => {
      if (line.startsWith("*** Update File: ")) {
        return line.slice("*** Update File: ".length).trim();
      }
      if (line.startsWith("*** Add File: ")) {
        return line.slice("*** Add File: ".length).trim();
      }
      if (line.startsWith("*** Delete File: ")) {
        return line.slice("*** Delete File: ".length).trim();
      }
      if (line.startsWith("*** Move to: ")) {
        return line.slice("*** Move to: ".length).trim();
      }
      return "";
    })
    .filter((s) => s.length > 0);
  return [...new Set(files)];
}

/**
 * Build an apply-patch-style block from antigravity's
 * `multi_replace_file_content` ReplacementChunks. Each chunk becomes one
 * `@@` hunk anchored at its `StartLine`. The shape matches what
 * `buildPatchLineDiff` already handles (the codex apply_patch format), so
 * we get the same numbered, syntax-highlighted rendering for free.
 */
function buildPatchFromAntigravityChunks(
  file: string,
  chunks: Array<Record<string, unknown>>,
): string {
  const safeFile = file || "file";
  const header = `*** Begin Patch\n*** Update File: ${safeFile}\n`;
  const hunks = chunks
    .map((chunk) => {
      const oldText = String(chunk.TargetContent ?? "");
      const newText = String(chunk.ReplacementContent ?? "");
      const oldLines = oldText.length === 0 ? [] : oldText.split("\n");
      const newLines = newText.length === 0 ? [] : newText.split("\n");
      const startLine = Number(chunk.StartLine ?? 1) || 1;
      const oldCount = Math.max(oldLines.length, 1);
      const newCount = Math.max(newLines.length, 1);
      const body = [
        ...oldLines.map((line) => `-${line}`),
        ...newLines.map((line) => `+${line}`),
      ].join("\n");
      return `@@ -${startLine},${oldCount} +${startLine},${newCount} @@\n${body}`;
    })
    .join("\n");
  return `${header}${hunks}\n*** End Patch\n`;
}

/** Format tool input for expanded view — structured, not raw JSON. */
export function formatToolInput(message: Message): ToolDetail | null {
  const name = message.tool_name || "";
  const inputJson = message.tool_input;
  if (!inputJson) return null;

  try {
    const obj = JSON.parse(inputJson) as Record<string, unknown>;
    switch (name) {
      case "Edit": {
        if (typeof obj.patch === "string") {
          const files = extractPatchedFiles(obj.patch);
          return {
            lines: [
              ...(files.length > 0
                ? [
                    {
                      label: "files",
                      value: files.map(shortenHomePath).join("\n"),
                    },
                  ]
                : [
                    toolLine(
                      "file",
                      obj.file_path || obj.filePath || obj.TargetFile || "",
                    ),
                  ]),
            ],
            patchDiff: buildPatchLineDiff(obj.patch),
          };
        }
        // Antigravity's `multi_replace_file_content` carries an array of
        // {TargetContent, ReplacementContent, StartLine, EndLine} chunks.
        // Concatenate them into a single unified-diff patch with one hunk
        // per chunk so the existing `patchDiff` renderer can display them.
        if (Array.isArray(obj.ReplacementChunks)) {
          const file =
            obj.TargetFile || obj.file_path || obj.filePath || "(unknown)";
          const patch = buildPatchFromAntigravityChunks(
            String(file),
            obj.ReplacementChunks as Array<Record<string, unknown>>,
          );
          return {
            lines: [toolLine("file", file)],
            patchDiff: buildPatchLineDiff(patch),
          };
        }
        return {
          lines: [
            toolLine(
              "file",
              obj.file_path || obj.filePath || obj.TargetFile || "",
            ),
          ],
          diff: {
            old: String(
              obj.old_string || obj.oldString || obj.TargetContent || "",
            ),
            new: String(
              obj.new_string || obj.newString || obj.ReplacementContent || "",
            ),
          },
        };
      }
      case "Write":
        return {
          lines: [
            toolLine(
              "file",
              obj.file_path || obj.filePath || obj.TargetFile || "",
            ),
            {
              label: "content",
              value: String(
                obj.content || obj.CodeContent || obj.code_content || "",
              ),
            },
          ],
        };
      case "Read":
      case "ReadMediaFile":
        return {
          lines: [
            toolLine(
              "file",
              obj.file_path ||
                obj.filePath ||
                obj.AbsolutePath ||
                obj.path ||
                "",
            ),
            ...(obj.offset
              ? [{ label: "offset", value: String(obj.offset) }]
              : []),
            ...(obj.limit
              ? [{ label: "limit", value: String(obj.limit) }]
              : []),
            ...(obj.StartLine
              ? [{ label: "start", value: String(obj.StartLine) }]
              : []),
            ...(obj.EndLine
              ? [{ label: "end", value: String(obj.EndLine) }]
              : []),
          ],
        };
      case "Bash":
        return {
          lines: [
            {
              label: "command",
              value: String(obj.command || obj.cmd || obj.CommandLine || ""),
            },
          ],
        };
      case "Plan": {
        const lines: Line[] = [];
        if (typeof obj.explanation === "string") {
          lines.push({ label: "explanation", value: obj.explanation });
        }
        if (Array.isArray(obj.plan)) {
          const planText = obj.plan
            .map((s) => {
              if (!s || typeof s !== "object") return "";
              const step = "step" in s ? String(s.step) : "";
              const status = "status" in s ? String(s.status) : "";
              const icon =
                status === "completed"
                  ? "✓"
                  : status === "in_progress"
                    ? "▸"
                    : "○";
              return `${icon} ${step}`;
            })
            .filter(Boolean)
            .join("\n");
          lines.push({ label: "plan", value: planText });
        }
        return { lines };
      }
      case "Grep":
        return {
          lines: [
            { label: "pattern", value: String(obj.pattern || obj.query || "") },
            ...(obj.path ? [toolLine("path", obj.path)] : []),
            ...(obj.glob ? [{ label: "glob", value: String(obj.glob) }] : []),
          ],
        };
      default:
        return {
          lines: Object.entries(obj)
            .filter(([, v]) => typeof v === "string" || typeof v === "number")
            .map(([k, v]) => toolLine(k, v))
            .slice(0, 8),
        };
    }
  } catch (error) {
    console.warn(`Failed to format tool input for ${name}:`, error);
    if (
      (name === "Apply_patch" || name === "Edit") &&
      inputJson.includes("*** Begin Patch")
    ) {
      const files = extractPatchedFiles(inputJson);
      return {
        lines: [
          ...(files.length > 0
            ? [{ label: "files", value: files.map(shortenHomePath).join("\n") }]
            : []),
        ],
        patchDiff: buildPatchLineDiff(inputJson),
      };
    }
    return { lines: [{ label: "raw", value: inputJson }] };
  }
}
