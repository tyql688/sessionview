import type { ToolMetadata } from "@/lib/types";

/** Result presentation is derived by Rust tool metadata builders. */
export function formatToolResultMetadata(metadata: ToolMetadata | undefined) {
  return metadata?.presentation?.resultDetail ?? null;
}
