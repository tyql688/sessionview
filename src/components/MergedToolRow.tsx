import { useState, useMemo } from "react";
import type { Message, Provider } from "../lib/types";
import { toolDisplayName, toolIcon } from "../lib/tools";
import { MessageBubble } from "./MessageBubble";

export function MergedToolRow(props: {
  tools: string[];
  messages: Message[];
  provider?: Provider;
  parentSessionId?: string;
  highlightTerm?: string;
}) {
  const [manualExpanded, setManualExpanded] = useState(false);
  const searchMatchesGroup = useMemo(() => {
    const term = (props.highlightTerm ?? "").trim().toLocaleLowerCase();
    if (!term) return false;
    return props.messages.some((message) =>
      [message.tool_name, message.tool_input, message.content]
        .filter((value): value is string => !!value)
        .some((value) => value.toLocaleLowerCase().includes(term)),
    );
  }, [props.highlightTerm, props.messages]);
  const expanded = manualExpanded || searchMatchesGroup;

  const label =
    props.tools.length > 0
      ? props.tools
          .map((toolName, index) => {
            const metadata = props.messages[index]?.tool_metadata;
            return `${toolIcon(toolName, metadata)} ${toolDisplayName(
              toolName,
              metadata,
            )}`;
          })
          .join(", ")
      : "tools";

  return (
    <div className="merged-tools">
      <div
        className="merged-tools-header"
        onClick={() => setManualExpanded((v) => !v)}
      >
        <span className="merged-tools-label">{label}</span>
        <span className="merged-tools-chevron">
          {expanded ? "\u25BE" : "\u25B8"}
        </span>
      </div>
      {expanded && (
        <div className="merged-tools-body">
          {props.messages.map((msg, i) => (
            <MessageBubble
              key={i}
              message={msg}
              provider={props.provider}
              parentSessionId={props.parentSessionId}
              highlightTerm={props.highlightTerm}
            />
          ))}
        </div>
      )}
    </div>
  );
}
