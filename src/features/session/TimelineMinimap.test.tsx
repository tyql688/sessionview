import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SessionTurnOutlineEntry } from "@/lib/tauri";
import {
  TimelineMinimap,
  activeTurnIndex,
} from "@/features/session/TimelineMinimap";

function outlineOf(count: number): SessionTurnOutlineEntry[] {
  return Array.from({ length: count }, (_, i) => ({
    ordinal: i,
    message_index: i * 2,
    user_text: `turn ${i}`,
    reply_text: "",
  }));
}

function renderMinimap(outline: SessionTurnOutlineEntry[], activeIndex = 0) {
  return render(
    <TimelineMinimap
      outline={outline}
      activeIndex={activeIndex}
      scrolling={false}
      onWheelScroll={() => {}}
      onRevealMessage={() => Promise.resolve(true)}
    />,
  );
}

describe("activeTurnIndex", () => {
  it("picks the last turn starting at or above the viewport-top message", () => {
    // Turns start at message indices 0/2/4/…; message 5 belongs to turn 2.
    expect(activeTurnIndex(outlineOf(4), 5, false)).toBe(2);
    expect(activeTurnIndex(outlineOf(4), 0, false)).toBe(0);
  });

  it("points at the last turn when the newest row is visible", () => {
    // Bottom of the session: the viewport top may still show an older turn,
    // but the user is reading the newest one.
    expect(activeTurnIndex(outlineOf(4), 2, true)).toBe(3);
  });

  it("defaults to the newest turn while nothing is measurable yet", () => {
    expect(activeTurnIndex(outlineOf(4), null, false)).toBe(3);
    expect(activeTurnIndex([], 10, false)).toBe(0);
  });
});

describe("TimelineMinimap", () => {
  it("renders one tick per outline turn — the whole session, no sampling", () => {
    const { container } = renderMinimap(outlineOf(200));
    expect(container.querySelectorAll(".timeline-minimap-tick")).toHaveLength(
      200,
    );
  });

  it("hides below the minimum turn count", () => {
    const { container } = renderMinimap(outlineOf(1));
    expect(container.querySelector(".timeline-minimap")).toBeNull();
  });

  it("marks the active tick", () => {
    const { container } = renderMinimap(outlineOf(5), 3);
    const ticks = container.querySelectorAll(".timeline-minimap-tick");
    expect(ticks[3]?.classList.contains("timeline-minimap-tick-active")).toBe(
      true,
    );
    expect(ticks[0]?.classList.contains("timeline-minimap-tick-active")).toBe(
      false,
    );
  });
});
