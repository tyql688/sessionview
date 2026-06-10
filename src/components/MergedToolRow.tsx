import { createSignal, createMemo, Show, For } from "solid-js";
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
  const [manualExpanded, setManualExpanded] = createSignal(false);
  const searchMatchesGroup = createMemo(() => {
    const term = (props.highlightTerm ?? "").trim().toLocaleLowerCase();
    if (!term) return false;
    return props.messages.some((message) =>
      [message.tool_name, message.tool_input, message.content]
        .filter((value): value is string => !!value)
        .some((value) => value.toLocaleLowerCase().includes(term)),
    );
  });
  const expanded = () => manualExpanded() || searchMatchesGroup();

  const label = () =>
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
    <div class="merged-tools">
      <div
        class="merged-tools-header"
        onClick={() => setManualExpanded((v) => !v)}
      >
        <span class="merged-tools-label">{label()}</span>
        <span class="merged-tools-chevron">
          {expanded() ? "\u25BE" : "\u25B8"}
        </span>
      </div>
      <Show when={expanded()}>
        <div class="merged-tools-body">
          <For each={props.messages}>
            {(msg) => (
              <MessageBubble
                message={msg}
                provider={props.provider}
                parentSessionId={props.parentSessionId}
                highlightTerm={props.highlightTerm}
              />
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
