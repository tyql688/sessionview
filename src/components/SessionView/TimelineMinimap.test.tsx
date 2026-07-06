import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SessionTurnOutlineEntry } from "../../lib/tauri";
import { TimelineMinimap } from "./TimelineMinimap";

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
