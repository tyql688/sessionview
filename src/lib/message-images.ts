/** An inline image extracted from message content. `source` is the original
 * path/URL from the `[Image: source: …]` placeholder, or null for bare
 * `[Image]` markers whose source the provider did not record. */
export interface ImageRef {
  source: string | null;
}

const IMAGE_PLACEHOLDER_RE =
  /\[Image(?:\s*#\d+)?(?::\s*source:\s*([^\]]+))?\]/g;

/** Split message content into markdown text and the image refs embedded in
 * it. The backend embeds images as `[Image: source: …]` text placeholders,
 * which the markdown renderer does not understand — they must be stripped
 * and rendered separately. */
export function extractImages(content: string): {
  markdown: string;
  images: ImageRef[];
} {
  if (!content.includes("[Image")) {
    return { markdown: content, images: [] };
  }
  const images: ImageRef[] = [];
  const markdown = content
    .replace(IMAGE_PLACEHOLDER_RE, (_match, source: string | undefined) => {
      images.push({ source: source?.trim() ?? null });
      return "";
    })
    .trim();
  return { markdown, images };
}
