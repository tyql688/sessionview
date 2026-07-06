import { useState } from "react";
import type { Message, Provider } from "@/lib/types";
import { toolDisplayName, toolIcon } from "@/lib/tools";
import { MessageBubble } from "@/features/session/MessageBubble";

export function MergedToolRow(props: {
  tools: string[];
  messages: Message[];
  provider?: Provider;
  parentSessionId?: string;
}) {
  const [expanded, setManualExpanded] = useState(false);

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
            />
          ))}
        </div>
      )}
    </div>
  );
}
