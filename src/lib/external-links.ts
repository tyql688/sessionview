import { invoke } from "@tauri-apps/api/core";

const EXTERNAL_URL_PROTOCOLS = new Set(["http:", "https:", "mailto:", "tel:"]);

export function normalizeExternalUrl(rawUrl: string): string {
  let parsed: URL;
  try {
    parsed = new URL(rawUrl);
  } catch (error) {
    throw new Error(`Invalid external URL: ${rawUrl}`, { cause: error });
  }

  if (!EXTERNAL_URL_PROTOCOLS.has(parsed.protocol)) {
    throw new Error(`Unsupported external URL protocol: ${parsed.protocol}`);
  }

  return parsed.href;
}

export function isExternalUrl(rawUrl: string): boolean {
  try {
    normalizeExternalUrl(rawUrl);
    return true;
  } catch (error) {
    console.warn("Rejected markdown link URL:", error);
    return false;
  }
}

export async function openExternalUrl(rawUrl: string): Promise<void> {
  const url = normalizeExternalUrl(rawUrl);
  await invoke<void>("plugin:opener|open_url", { url });
}
