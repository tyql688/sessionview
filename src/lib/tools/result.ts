import type { ToolMetadata } from "@/lib/types";
import type { ToolDetail } from "@/lib/tools/types";

/** Result presentation is derived by Rust tool metadata builders. */
export function formatToolResultMetadata(metadata: ToolMetadata | undefined): ToolDetail | null {
  return metadata?.presentation?.resultDetail ?? null;
}
