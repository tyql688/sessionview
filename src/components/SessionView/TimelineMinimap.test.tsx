import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SessionTurnOutlineEntry } from "@/lib/tauri";
import {
  TimelineMinimap,
  currentTickFromOffsets,
  sampleOutline,
} from "@/components/SessionView/TimelineMinimap";

function outlineOf(count: number): SessionTurnOutlineEntry[] {
  return Array.from({ length: count }, (_, i) => ({
    ordinal: i,
    message_index: i * 2,
    user_text: `turn ${i}`,
    reply_text: "",
  }));
}

function makeScroller(): HTMLDivElement {
  const el = document.createElement("div");
  document.body.appendChild(el);
  return el;
}

describe("sampleOutline", () => {
  it("returns the outline unchanged when under the cap", () => {
    const outline = outlineOf(5);
    expect(sampleOutline(outline, 32)).toBe(outline);
  });

  it("samples long outlines down to the cap, keeping first and last", () => {
    const sampled = sampleOutline(outlineOf(200), 32);
    expect(sampled).toHaveLength(32);
    expect(sampled[0]?.ordinal).toBe(0);
    expect(sampled[sampled.length - 1]?.ordinal).toBe(199);
    const ordinals = sampled.map((t) => t.ordinal);
    expect(new Set(ordinals).size).toBe(ordinals.length);
  });
});

describe("currentTickFromOffsets", () => {
  it("picks the last anchor scrolled past the viewport top", () => {
    // Normal-scroll coordinates: anchors at 0/500/1000, viewport at 520.
    expect(currentTickFromOffsets([0, 500, 1000], 520)).toBe(1);
  });

  it("works with the column-reverse negative coordinate space", () => {
    // Bottom of a column-reverse scroller: scrollTop 0, older anchors above
    // at negative offsets, the newest turn's anchor just above the top.
    expect(currentTickFromOffsets([-900, -400, -10], 0)).toBe(2);
    // Scrolled up into history.
    expect(currentTickFromOffsets([-900, -400, -10], -450)).toBe(0);
  });

  it("skips unmounted anchors instead of stopping at them", () => {
    // Tail-first loading: the oldest turns have no DOM node (Infinity) while
    // newer ones do — they must not mask the real current turn.
    const inf = Number.POSITIVE_INFINITY;
    expect(currentTickFromOffsets([inf, inf, -400, -10], 0)).toBe(3);
  });
});

describe("TimelineMinimap", () => {
  it("renders one tick per outline turn", () => {
    const { container } = render(
      <TimelineMinimap
        outline={outlineOf(3)}
        messagesRef={makeScroller()}
        onRevealMessage={() => Promise.resolve(true)}
      />,
    );
    expect(container.querySelectorAll(".timeline-minimap-tick")).toHaveLength(
      3,
    );
  });

  it("hides below the minimum turn count", () => {
    const { container } = render(
      <TimelineMinimap
        outline={outlineOf(1)}
        messagesRef={makeScroller()}
        onRevealMessage={() => Promise.resolve(true)}
      />,
    );
    expect(container.querySelector(".timeline-minimap")).toBeNull();
  });

  // Regression: the scroll container mounts after the outline arrives, so the
  // element prop starts undefined. The subscription effect must re-run and
  // attach once the element is finally passed in — the migration originally
  // handed a render-time ref snapshot down, which never updated and left the
  // minimap without a scroll listener.
  it("attaches the scroll listener when the container arrives late", () => {
    const scroller = makeScroller();
    const addSpy = vi.spyOn(scroller, "addEventListener");

    const { rerender, unmount } = render(
      <TimelineMinimap
        outline={outlineOf(3)}
        messagesRef={null}
        onRevealMessage={() => Promise.resolve(true)}
      />,
    );
    expect(addSpy).not.toHaveBeenCalled();

    rerender(
      <TimelineMinimap
        outline={outlineOf(3)}
        messagesRef={scroller}
        onRevealMessage={() => Promise.resolve(true)}
      />,
    );
    expect(addSpy).toHaveBeenCalledWith("scroll", expect.any(Function), {
      passive: true,
    });

    const removeSpy = vi.spyOn(scroller, "removeEventListener");
    unmount();
    expect(removeSpy).toHaveBeenCalledWith("scroll", expect.any(Function));
  });
});
