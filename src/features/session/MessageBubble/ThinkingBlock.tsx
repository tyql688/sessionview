import { useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
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
      <button
        type="button"
        className="msg-thinking-header"
        aria-expanded={expanded}
        onClick={() => setExpanded(!expanded)}
      >
        <span className="msg-thinking-icon">💭</span>
        <span className="msg-thinking-label">{t("timeline.thinking")}</span>
        {!expanded && <span className="msg-thinking-preview">{preview()}</span>}
        <span className="msg-thinking-chevron" aria-hidden="true">
          {expanded ? <ChevronDown size={12} strokeWidth={1.75} /> : <ChevronRight size={12} strokeWidth={1.75} />}
        </span>
      </button>
      {expanded && <pre className="msg-thinking-content">{props.content}</pre>}
    </div>
  );
}
