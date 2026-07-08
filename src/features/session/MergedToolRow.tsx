import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import { useI18n } from "@/i18n/index";
import type { Message, Provider } from "@/lib/types";
import { toolDisplayName } from "@/lib/tools";
import { MessageBubble } from "@/features/session/MessageBubble";
import { ToolKindGlyph, toolVisualKind } from "@/features/session/ToolGlyph";

interface ToolGroup {
  key: string;
  category: string;
  canonicalName: string;
  label: string;
  count: number;
}

const VISIBLE_GROUPS = 3;

function toolGroups(messages: Message[], tools: string[]): ToolGroup[] {
  const items =
    messages.length > 0
      ? messages.map((message, index) => ({
          name: message.tool_name || tools[index] || message.tool_metadata?.canonical_name || "Tool",
          metadata: message.tool_metadata,
        }))
      : tools.map((name) => ({ name, metadata: undefined }));

  const groups: ToolGroup[] = [];
  const byKey = new Map<string, ToolGroup>();
  for (const item of items) {
    const label = toolDisplayName(item.name, item.metadata);
    const { canonicalName, category } = toolVisualKind(item.name, item.metadata);
    const key = `${canonicalName}:${item.metadata?.mcp?.server ?? ""}:${label}`;
    const existing = byKey.get(key);
    if (existing) {
      existing.count += 1;
      continue;
    }
    const group = { key, category, canonicalName, label, count: 1 };
    byKey.set(key, group);
    groups.push(group);
  }
  return groups;
}

export function MergedToolRow(props: {
  tools: string[];
  messages: Message[];
  provider?: Provider;
  parentSessionId?: string;
}) {
  const { t } = useI18n();
  const [expanded, setManualExpanded] = useState(false);
  const groups = toolGroups(props.messages, props.tools);
  const visibleGroups = groups.slice(0, VISIBLE_GROUPS);
  const hiddenGroupCount = Math.max(0, groups.length - visibleGroups.length);
  const toolCount = props.messages.length || props.tools.length;

  return (
    <div className="merged-tools">
      <button
        type="button"
        className="merged-tools-header"
        onClick={() => setManualExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        {expanded ? (
          <ChevronDown className="merged-tools-chevron" aria-hidden="true" />
        ) : (
          <ChevronRight className="merged-tools-chevron" aria-hidden="true" />
        )}
        <span className="merged-tools-count">{t("tool.groupCount", { count: toolCount })}</span>
        <span className="merged-tools-divider" aria-hidden="true" />
        <span className="merged-tools-strip">
          {visibleGroups.map((group) => (
            <span className="merged-tools-token" key={group.key}>
              <ToolKindGlyph
                canonicalName={group.canonicalName}
                category={group.category}
                className="merged-tools-token-icon"
              />
              <span className="merged-tools-token-label">{group.label}</span>
              {group.count > 1 && <span className="merged-tools-token-count">x{group.count}</span>}
            </span>
          ))}
          {hiddenGroupCount > 0 && (
            <span className="merged-tools-overflow">{t("tool.groupMore", { count: hiddenGroupCount })}</span>
          )}
        </span>
      </button>
      {expanded && (
        <div className="merged-tools-body">
          {props.messages.map((msg, i) => (
            <MessageBubble key={i} message={msg} provider={props.provider} parentSessionId={props.parentSessionId} />
          ))}
        </div>
      )}
    </div>
  );
}
