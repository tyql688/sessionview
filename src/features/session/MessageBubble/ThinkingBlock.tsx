import { useState } from "react";

export function ThinkingBlock(props: { content: string }) {
  const [expanded, setExpanded] = useState(false);
  const preview = () => {
    const first = props.content.split("\n")[0];
    return first.length > 80 ? `${first.slice(0, 80)}...` : first;
  };

  return (
    <div className={`msg-thinking${expanded ? " expanded" : ""}`}>
      <div
        className="msg-thinking-header"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="msg-thinking-icon">💭</span>
        <span className="msg-thinking-label">Thinking</span>
        {!expanded && <span className="msg-thinking-preview">{preview()}</span>}
        <span className="msg-thinking-chevron">{expanded ? "▾" : "▸"}</span>
      </div>
      {expanded && <pre className="msg-thinking-content">{props.content}</pre>}
    </div>
  );
}
