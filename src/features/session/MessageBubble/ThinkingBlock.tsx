import { useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useI18n } from "@/i18n/index";

export function ThinkingBlock(props: { content: string }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const preview = () => {
    // A bold lead ("**Title** …") is the model's own headline — show just
    // it. Otherwise the first line stands in, markdown-ish **emphasis**
    // stripped, hard-capped so an untitled wall of thought stays one line.
    const firstLine = props.content.split("\n")[0].trim();
    const bold = /^\*\*([^*]+)\*\*/.exec(firstLine);
    const summary = (bold ? bold[1] : firstLine.replaceAll("**", "")).trim();
    return summary.length > 80 ? `${summary.slice(0, 80)}\u2026` : summary;
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
