import { diffLines } from "diff";
import type { ToolDiffLine, ToolDiffLineType } from "@/lib/types";

function stripTrailingNewline(line: string): string {
  return line.endsWith("\n") ? line.slice(0, -1) : line;
}

function pushLine(
  lines: ToolDiffLine[],
  type: Exclude<ToolDiffLineType, "skip">,
  text: string,
  oldLine: number | null,
  newLine: number | null,
) {
  lines.push({
    type,
    oldLine,
    newLine,
    text: stripTrailingNewline(text),
  });
}

export function buildToolLineDiff(oldText: string, newText: string): ToolDiffLine[] {
  const lines: ToolDiffLine[] = [];
  let oldLine = 1;
  let newLine = 1;

  for (const part of diffLines(oldText, newText)) {
    const rawLines = part.value.match(/[^\n]*\n|[^\n]+/g) ?? [];
    for (const rawLine of rawLines) {
      if (part.added) {
        pushLine(lines, "add", rawLine, null, newLine);
        newLine += 1;
      } else if (part.removed) {
        pushLine(lines, "remove", rawLine, oldLine, null);
        oldLine += 1;
      } else {
        pushLine(lines, "context", rawLine, oldLine, newLine);
        oldLine += 1;
        newLine += 1;
      }
    }
  }

  return lines;
}

export interface InlineSegment {
  text: string;
  changed: boolean;
}

/**
 * Character-level emphasis for a paired remove/add line: the common prefix
 * and suffix stay quiet, the differing middle gets highlighted. Prefix/suffix
 * trimming (instead of a full LCS) is O(n), stable, and matches how humans
 * read single-line edits; disjoint rewrites degrade gracefully to one full
 * changed segment.
 */
export function inlineSegments(from: string, to: string): { from: InlineSegment[]; to: InlineSegment[] } {
  let prefix = 0;
  const max = Math.min(from.length, to.length);
  while (prefix < max && from[prefix] === to[prefix]) prefix += 1;

  let suffix = 0;
  while (suffix < max - prefix && from[from.length - 1 - suffix] === to[to.length - 1 - suffix]) {
    suffix += 1;
  }

  const segments = (line: string): InlineSegment[] => {
    const head = line.slice(0, prefix);
    const mid = line.slice(prefix, line.length - suffix);
    const tail = line.slice(line.length - suffix);
    const out: InlineSegment[] = [];
    if (head) out.push({ text: head, changed: false });
    if (mid) out.push({ text: mid, changed: true });
    if (tail) out.push({ text: tail, changed: false });
    return out.length > 0 ? out : [{ text: line, changed: false }];
  };

  return { from: segments(from), to: segments(to) };
}

/**
 * Pair up adjacent remove/add runs of equal position so the renderer can
 * apply character-level emphasis: run of N removes followed by run of M adds
 * pairs index-by-index for min(N, M) lines.
 */
export function pairChangedLines(lines: ToolDiffLine[]): Map<number, number> {
  const pairs = new Map<number, number>();
  let i = 0;
  while (i < lines.length) {
    if (lines[i]?.type !== "remove") {
      i += 1;
      continue;
    }
    const removeStart = i;
    while (i < lines.length && lines[i]?.type === "remove") i += 1;
    const addStart = i;
    while (i < lines.length && lines[i]?.type === "add") i += 1;
    const removeCount = addStart - removeStart;
    const addCount = i - addStart;
    for (let k = 0; k < Math.min(removeCount, addCount); k += 1) {
      pairs.set(removeStart + k, addStart + k);
      pairs.set(addStart + k, removeStart + k);
    }
  }
  return pairs;
}
