import { useMemo, type ReactNode } from "react";
import { buildToolLineDiff, inlineSegments, pairChangedLines } from "@/lib/diff";
import type { ToolDiffLine } from "@/lib/types";

function diffKey(line: ToolDiffLine, index: number): string {
  return `${index}:${line.type}:${line.oldLine ?? ""}:${line.newLine ?? ""}:${line.text}`;
}

function markerFor(type: ToolDiffLine["type"]): string {
  switch (type) {
    case "add":
      return "+";
    case "remove":
      return "-";
    case "context":
      return " ";
    case "skip":
      return "⋯";
  }
}

function lineNumberFor(line: ToolDiffLine): number | null {
  if (line.type === "remove") return line.oldLine;
  return line.newLine ?? line.oldLine;
}

export function SessionDiffView(props: { lines: ToolDiffLine[] }) {
  const pairs = useMemo(() => pairChangedLines(props.lines), [props.lines]);
  const added = props.lines.filter((line) => line.type === "add").length;
  const removed = props.lines.filter((line) => line.type === "remove").length;

  const codeFor = (line: ToolDiffLine, index: number): ReactNode => {
    const fallback = line.text || " ";
    const partner = pairs.get(index);
    if (partner === undefined) return fallback;

    const other = props.lines[partner];
    if (!other) return fallback;

    const { from, to } = inlineSegments(
      line.type === "remove" ? line.text : other.text,
      line.type === "remove" ? other.text : line.text,
    );
    const segments = line.type === "remove" ? from : to;

    return segments.map((segment, segmentIndex) =>
      segment.changed ? (
        <mark className={`msg-tool-diff-emph ${line.type}`} key={segmentIndex}>
          {segment.text}
        </mark>
      ) : (
        segment.text
      ),
    );
  };

  return (
    <div className="msg-tool-line-diff">
      <div className="msg-tool-diff-toolbar">
        <div className="msg-tool-diff-line-labels" aria-hidden="true">
          <span>#</span>
        </div>
        <div className="msg-tool-diff-stats">
          <span className="msg-tool-diff-stat add">+{added}</span>
          <span className="msg-tool-diff-stat remove">-{removed}</span>
        </div>
      </div>
      <div className="msg-tool-diff-body">
        {props.lines.map((line, index) =>
          line.type === "skip" ? (
            <div className="msg-tool-diff-section" key={diffKey(line, index)}>
              <span className="msg-tool-diff-section-marker">{markerFor(line.type)}</span>
              <span className="msg-tool-diff-section-text">{line.text || "⋯"}</span>
            </div>
          ) : (
            <div className={`msg-tool-diff-line ${line.type}`} key={diffKey(line, index)}>
              <span className="msg-tool-diff-gutter">{lineNumberFor(line) ?? ""}</span>
              <span className="msg-tool-diff-marker">{markerFor(line.type)}</span>
              <span className="msg-tool-diff-code">{codeFor(line, index)}</span>
            </div>
          ),
        )}
      </div>
    </div>
  );
}

export function SessionLineDiffView(props: { oldText: string; newText: string }) {
  const lines = useMemo(() => buildToolLineDiff(props.oldText, props.newText), [props.oldText, props.newText]);
  return <SessionDiffView lines={lines} />;
}
