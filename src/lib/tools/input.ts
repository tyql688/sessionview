import type { Message } from "@/lib/types";
import type { ToolDetail } from "@/lib/tools/types";

/** Input presentation is derived by Rust tool metadata builders. */
export function formatToolInput(message: Message): ToolDetail | null {
  return message.tool_metadata?.presentation?.inputDetail ?? null;
}
