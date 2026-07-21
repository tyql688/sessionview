/** An inline image extracted from message content. `source` is the original
 * path/URL from the `[Image: source: …]` placeholder, or null for bare
 * `[Image]` markers whose source the provider did not record. */
export interface ImageRef {
  source: string | null;
}

const IMAGE_PLACEHOLDER_RE = /\[Image(?:\s*#\d+)?(?::\s*source:\s*([^\]]+))?\]/g;

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

/** Neutralize image placeholders for plain-text copy: sources are local
 * paths/URLs that mean nothing outside the app. */
export function sanitizeMessageForClipboard(raw: string): string {
  return raw.replace(IMAGE_PLACEHOLDER_RE, "[Image]");
}

/** A slice of raw tool output: prose, a fenced code block, or an image ref. */
export interface ContentSegment {
  type: "text" | "code" | "image";
  content: string;
  language?: string;
}

/** Split raw tool output into text / fenced-code / image segments. Tool
 * output is NOT markdown — only ``` fences and image placeholders get
 * structure; everything else stays verbatim text. */
export function parseContent(
  raw: string,
  structuredImageSources: readonly string[] = [],
  options: { parseCodeFences?: boolean } = {},
): ContentSegment[] {
  const parseCodeFences = options.parseCodeFences !== false;
  if ((!parseCodeFences || !raw.includes("```")) && !raw.includes("[Image") && structuredImageSources.length === 0) {
    return [{ type: "text", content: raw }];
  }

  const segments: ContentSegment[] = [];
  const blockRegex = parseCodeFences
    ? /```([\w+#.-]*)\n?([\s\S]*?)```|\[Image(?:\s*#\d+)?(?::\s*source:\s*([^\]]+))?\]/g
    : /\[Image(?:\s*#\d+)?(?::\s*source:\s*([^\]]+))?\]/g;
  let lastIndex = 0;
  let structuredImageIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = blockRegex.exec(raw)) !== null) {
    if (match.index > lastIndex) {
      segments.push({
        type: "text",
        content: raw.slice(lastIndex, match.index),
      });
    }

    if (parseCodeFences && match[2] !== undefined) {
      segments.push({
        type: "code",
        content: match[2],
        language: match[1] || undefined,
      });
    } else {
      const explicitImagePath = parseCodeFences ? match[3] : match[1];
      const imagePath = explicitImagePath?.trim() || structuredImageSources[structuredImageIndex];
      if (structuredImageIndex < structuredImageSources.length) structuredImageIndex += 1;
      if (imagePath) {
        segments.push({ type: "image", content: imagePath });
      } else {
        segments.push({ type: "text", content: match[0] });
      }
    }

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < raw.length) {
    segments.push({ type: "text", content: raw.slice(lastIndex) });
  }

  for (; structuredImageIndex < structuredImageSources.length; structuredImageIndex += 1) {
    const source = structuredImageSources[structuredImageIndex];
    if (source) segments.push({ type: "image", content: source });
  }

  return segments;
}
