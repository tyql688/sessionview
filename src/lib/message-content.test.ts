import { describe, expect, it } from "vitest";
import {
  extractImages,
  parseContent,
  sanitizeMessageForClipboard,
} from "@/lib/message-content";

describe("extractImages", () => {
  it("pulls sourced and bare placeholders out of the markdown", () => {
    const { markdown, images } = extractImages(
      "look at [Image #1: source: /tmp/a.png] and [Image] here",
    );
    expect(markdown).toBe("look at  and  here");
    expect(images).toEqual([{ source: "/tmp/a.png" }, { source: null }]);
  });

  it("leaves image-free content untouched", () => {
    expect(extractImages("plain text")).toEqual({
      markdown: "plain text",
      images: [],
    });
  });
});

describe("parseContent", () => {
  it("keeps fenced code whitespace while still splitting images", () => {
    const segments = parseContent(
      "```ts\n\nconst value = 1;\n```\n[Image: source: /tmp/diagram.png]",
    );

    expect(segments).toHaveLength(3);
    expect(segments[0]).toMatchObject({
      type: "code",
      language: "ts",
    });
    expect(segments[0]?.content.startsWith("\n")).toBe(true);
    expect(segments[1]).toEqual({
      type: "text",
      content: "\n",
    });
    expect(segments[2]).toEqual({
      type: "image",
      content: "/tmp/diagram.png",
    });
  });

  it("returns plain output as a single text segment", () => {
    expect(parseContent("just stdout")).toEqual([
      { type: "text", content: "just stdout" },
    ]);
  });

  it("hydrates bare image markers from typed structured output", () => {
    expect(parseContent("captured\n[Image]", ["data:image/png;base64,AAAA"])).toEqual([
      { type: "text", content: "captured\n" },
      { type: "image", content: "data:image/png;base64,AAAA" },
    ]);
  });

  it("does not duplicate a sourced marker when structured output carries the same media slot", () => {
    expect(parseContent("[Image: source: /tmp/local.png]", ["blobref:image/png;abc"])).toEqual([
      { type: "image", content: "/tmp/local.png" },
    ]);
  });

  it("appends typed images when provider text has no placeholder", () => {
    expect(parseContent("captured", ["https://example.com/shot.png"])).toEqual([
      { type: "text", content: "captured" },
      { type: "image", content: "https://example.com/shot.png" },
    ]);
  });

  it("preserves code fences when parsing a terminal stream", () => {
    const terminalOutput = "```json\n{\"keep\":true}\n```\n[Image]";
    expect(
      parseContent(terminalOutput, ["data:image/png;base64,AAAA"], {
        parseCodeFences: false,
      }),
    ).toEqual([
      { type: "text", content: "```json\n{\"keep\":true}\n```\n" },
      { type: "image", content: "data:image/png;base64,AAAA" },
    ]);
  });
});

describe("sanitizeMessageForClipboard", () => {
  it("normalizes numbered image placeholders", () => {
    expect(
      sanitizeMessageForClipboard(
        "Before [Image #1: source: /tmp/screenshot.png] after [Image #2]",
      ),
    ).toBe("Before [Image] after [Image]");
  });
});
