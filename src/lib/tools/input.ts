import type { Message } from "@/lib/types";

/** Input presentation is derived by Rust tool metadata builders. */
export function formatToolInput(message: Message) {
  return message.tool_metadata?.presentation?.inputDetail ?? null;
}
