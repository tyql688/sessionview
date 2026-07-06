import { invoke } from "@tauri-apps/api/core";

const EXTERNAL_URL_PROTOCOLS = new Set(["http:", "https:", "mailto:", "tel:"]);

function normalizeExternalUrl(rawUrl: string): string {
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

export async function openExternalUrl(rawUrl: string): Promise<void> {
  const url = normalizeExternalUrl(rawUrl);
  await invoke<void>("plugin:opener|open_url", { url });
}
