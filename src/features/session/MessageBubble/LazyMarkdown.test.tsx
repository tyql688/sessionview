// @vitest-environment happy-dom
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LazyMarkdown } from "@/features/session/MessageBubble/LazyMarkdown";

vi.mock("@/features/session/timeline/Markdown", () => ({
  Markdown: (props: { text: string }) => (
    <div data-testid="real-markdown">{props.text}</div>
  ),
}));

// LazyMarkdown creates ONE shared IntersectionObserver for the whole module,
// so the fake must survive across tests: the callback captured at first
// construction stays valid for every later observe().
let lastIntersect: IntersectionObserverCallback | null = null;
let lastObserver: IntersectionObserver | null = null;
let observedElements: Element[] = [];

class FakeIntersectionObserver implements IntersectionObserver {
  readonly root = null;
  readonly rootMargin = "";
  readonly thresholds: readonly number[] = [];
  constructor(callback: IntersectionObserverCallback) {
    lastIntersect = callback;
    lastObserver = this;
  }
  observe(el: Element) {
    observedElements.push(el);
  }
  unobserve() {}
  disconnect() {}
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }
}

function fireIntersect(target: Element) {
  if (!lastIntersect || !lastObserver) throw new Error("no observer created");
  const entry = { isIntersecting: true, target } as IntersectionObserverEntry;
  lastIntersect([entry], lastObserver);
}

vi.stubGlobal("IntersectionObserver", FakeIntersectionObserver);

beforeEach(() => {
  observedElements = [];
});

describe("LazyMarkdown", () => {
  it("renders plain text until the row nears the viewport", () => {
    render(<LazyMarkdown text="**bold** stays raw" />);

    expect(screen.getByText("**bold** stays raw")).toBeTruthy();
    expect(screen.queryByTestId("real-markdown")).toBeNull();
    expect(observedElements.length).toBeGreaterThan(0);
  });

  it("upgrades to real markdown when the observer reports intersection", async () => {
    render(<LazyMarkdown text="upgrade me" />);
    expect(screen.queryByTestId("real-markdown")).toBeNull();

    fireIntersect(observedElements[observedElements.length - 1]);

    await waitFor(() => {
      expect(screen.getByTestId("real-markdown")).toBeTruthy();
    });
  });

  it("skips the gate entirely for eager rows", () => {
    render(<LazyMarkdown text="on-screen tail" eager />);

    expect(screen.getByTestId("real-markdown")).toBeTruthy();
  });
});
