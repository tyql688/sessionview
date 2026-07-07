import { lazy, Suspense, useEffect, useRef, useState } from "react";

// The markdown engine (streamdown + shiki/katex/mermaid plugins) is by far
// the heaviest frontend dependency — load it on demand so the app shell and
// explorer render without it. The fallback shows the raw text, so the brief
// first-load gap still reads.
const Markdown = lazy(() =>
  import("@/features/session/timeline/Markdown").then((module) => ({
    default: module.Markdown,
  })),
);

/** How far outside the viewport a bubble may sit and still get real markdown.
 * Generous so fast scrolling and search jumps land on upgraded rows. */
const NEAR_VIEWPORT_MARGIN = "1600px";

type OnNear = () => void;

let sharedObserver: IntersectionObserver | null = null;
const nearCallbacks = new WeakMap<Element, OnNear>();

/** One IntersectionObserver for every bubble: fires each subscriber once when
 * its element approaches the viewport, then unobserves. Returns an unsubscribe
 * for unmounts that happen before the upgrade. */
function observeNearViewport(el: Element, onNear: OnNear): () => void {
  sharedObserver ??= new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const callback = nearCallbacks.get(entry.target);
        nearCallbacks.delete(entry.target);
        sharedObserver?.unobserve(entry.target);
        callback?.();
      }
    },
    { rootMargin: NEAR_VIEWPORT_MARGIN },
  );
  nearCallbacks.set(el, onNear);
  sharedObserver.observe(el);
  return () => {
    nearCallbacks.delete(el);
    sharedObserver?.unobserve(el);
  };
}

interface LazyMarkdownProps {
  text: string;
  /** Skip the near-viewport gate — for rows known to be on screen at mount
   * (the newest tail on open), so the first paint is real markdown. */
  eager?: boolean;
}

/**
 * Markdown that renders as cheap plain text until the bubble nears the
 * viewport, then upgrades to the full streamdown pipeline exactly once.
 *
 * Windowed loading mounts entries 80 at a time while the user scrolls; parsing
 * + highlighting all of them synchronously froze the frame on long (codex)
 * sessions. Off-screen rows keep the full text in the DOM, so in-session
 * search ranges and minimap anchors still work — they just see raw markdown
 * until the row is approached, and search re-collects ranges from the live
 * DOM on every navigation.
 */
export function LazyMarkdown(props: LazyMarkdownProps) {
  const holderRef = useRef<HTMLDivElement | null>(null);
  const [near, setNear] = useState(false);
  // Environments without IntersectionObserver (tests) render markdown
  // directly — the gate is a pure optimization, never a behavior switch.
  const ready =
    props.eager || near || typeof IntersectionObserver === "undefined";

  useEffect(() => {
    if (ready) return;
    const el = holderRef.current;
    if (!el) return;
    return observeNearViewport(el, () => setNear(true));
  }, [ready]);

  if (!ready) {
    return (
      <div ref={holderRef} className="whitespace-pre-wrap">
        {props.text}
      </div>
    );
  }
  return (
    <Suspense
      fallback={<div className="whitespace-pre-wrap">{props.text}</div>}
    >
      <Markdown text={props.text} />
    </Suspense>
  );
}
