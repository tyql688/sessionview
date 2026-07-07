import { useState } from "react";
import { useI18n } from "@/i18n/index";

export function ThinkingBlock(props: { content: string }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const preview = () => {
    // Models pepper thinking with markdown-ish **emphasis**; the collapsed
    // one-liner reads better with the markers stripped (content stays raw).
    const first = props.content.split("\n")[0].replaceAll("**", "");
    return first;
  };

  return (
    <div className={`msg-thinking${expanded ? " expanded" : ""}`}>
      <div
        className="msg-thinking-header"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="msg-thinking-icon">💭</span>
        <span className="msg-thinking-label">{t("timeline.thinking")}</span>
        {!expanded && <span className="msg-thinking-preview">{preview()}</span>}
        <span className="msg-thinking-chevron">{expanded ? "▾" : "▸"}</span>
      </div>
      {expanded && <pre className="msg-thinking-content">{props.content}</pre>}
    </div>
  );
}
