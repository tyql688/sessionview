import { useEffect, useMemo, useState } from "react";
import type { SessionTurnOutlineEntry } from "@/lib/tauri";

const MIN_TURNS_TO_SHOW = 2;
const SCROLL_REST_MS = 180;
/** How far past the viewport top an anchor may sit and still count as
 * "current" — covers scroll margins and sub-pixel rounding after a
 * tick-click scrollIntoView. */
const CURRENT_TURN_MARGIN_PX = 32;
/** Long sessions get evenly sampled ticks so the strip fits the viewport
 * instead of rendering one tick per turn. */
const MAX_TICKS = 32;

const MOUNTAIN = [
  { width: 24, className: "timeline-minimap-tick-peak" },
  { width: 18, className: "timeline-minimap-tick-near" },
  { width: 12, className: "timeline-minimap-tick-mid" },
  { width: 8, className: "timeline-minimap-tick-far" },
] as const;
const BASE_WIDTH = 5;

interface MinimapProps {
  outline: SessionTurnOutlineEntry[];
  /** Scroll container element; null until it mounts (state twin of the ref). */
  messagesRef: HTMLDivElement | null;
  onRevealMessage: (messageIndex: number) => Promise<boolean>;
}

function anchorFor(scroller: HTMLElement, ordinal: number): HTMLElement | null {
  return scroller.querySelector<HTMLElement>(`[data-turn="${ordinal}"]`);
}

/** DOM anchors per ordinal, resolved once per content change instead of per
 * scroll frame — 32 querySelector calls per frame showed up in traces. The
 * cache only stores found elements; missing ordinals stay re-resolvable
 * (their turns may enter the rendered window later). Detached elements are
 * re-resolved too (windowed loading swaps rows). */
class AnchorCache {
  private map = new Map<number, HTMLElement>();

  constructor(private scroller: HTMLElement) {}

  get(ordinal: number): HTMLElement | null {
    const cached = this.map.get(ordinal);
    if (cached?.isConnected) return cached;
    this.map.delete(ordinal);
    const found = anchorFor(this.scroller, ordinal);
    if (found) this.map.set(ordinal, found);
    return found;
  }
}

/** Evenly sample the outline down to `maxTicks` entries, always keeping the
 * first and last turns. */
export function sampleOutline(
  outline: SessionTurnOutlineEntry[],
  maxTicks: number,
): SessionTurnOutlineEntry[] {
  if (outline.length <= maxTicks || maxTicks < 2) return outline;
  const sampled: SessionTurnOutlineEntry[] = [];
  const step = (outline.length - 1) / (maxTicks - 1);
  for (let i = 0; i < maxTicks; i += 1) {
    const entry = outline[Math.round(i * step)];
    if (entry && sampled[sampled.length - 1] !== entry) sampled.push(entry);
  }
  return sampled;
}

/** Each sampled turn's anchor position in the scroller's scroll coordinate
 * space. These are scroll-invariant (they only move when content changes), so
 * callers cache the array and re-measure on invalidation instead of per scroll
 * frame — each call forces a synchronous layout per anchor, which was the
 * dominant scroll-jank source on huge sessions. Works unchanged for the
 * column-reverse container: scrollTop and offsets are both negative above the
 * bottom, and only their ordering matters. Missing anchors (turns outside the
 * rendered window) come in as Infinity. */
function anchorOffsets(
  scroller: HTMLElement,
  turns: SessionTurnOutlineEntry[],
  anchors: AnchorCache,
): number[] {
  const scrollerTop = scroller.getBoundingClientRect().top;
  return turns.map((turn) => {
    const anchor = anchors.get(turn.ordinal);
    if (!anchor) return Number.POSITIVE_INFINITY;
    return (
      anchor.getBoundingClientRect().top - scrollerTop + scroller.scrollTop
    );
  });
}

/** Scrollspy: the current tick is the LAST turn whose anchor has scrolled
 * past the viewport top. Reply lengths vary wildly, so proportional
 * (scroll-percentage) mapping points at turns the viewport isn't actually
 * showing — only anchor positions tell the truth. Missing anchors are
 * SKIPPED, not treated as a stop: the session loads tail-first, so older
 * turns routinely have no DOM node while newer ones do. */
export function currentTickFromOffsets(
  offsets: number[],
  scrollTop: number,
): number {
  // column-reverse pins scrollTop to 0 at the BOTTOM (latest messages) —
  // that state means "at the newest turn", so point at the last tick that
  // has a rendered anchor instead of running the top-anchor scan (which
  // degenerates to the first tick when every anchor sits inside/below the
  // viewport).
  if (Math.abs(scrollTop) < 4) {
    for (let i = offsets.length - 1; i >= 0; i -= 1) {
      const offset = offsets[i];
      if (offset !== undefined && offset !== Number.POSITIVE_INFINITY) {
        return i;
      }
    }
    return Math.max(0, offsets.length - 1);
  }

  let current = 0;
  for (let i = 0; i < offsets.length; i += 1) {
    const offset = offsets[i];
    if (offset === undefined || offset === Number.POSITIVE_INFINITY) continue;
    if (offset > scrollTop + CURRENT_TURN_MARGIN_PX) break;
    current = i;
  }
  return current;
}

export function TimelineMinimap(props: MinimapProps) {
  const turns = useMemo(
    () => sampleOutline(props.outline, MAX_TICKS),
    [props.outline],
  );
  const [active, setActive] = useState(0);
  const [hovered, setHovered] = useState<number | null>(null);
  const [scrolling, setScrolling] = useState(false);

  useEffect(() => {
    const scroller = props.messagesRef;
    if (!scroller || turns.length < MIN_TURNS_TO_SHOW) return;

    let frame = 0;
    let quiet: ReturnType<typeof setTimeout> | undefined;
    const anchors = new AnchorCache(scroller);

    // Anchor offsets are scroll-invariant, so the scroll path reads a cached
    // array and only `scrollTop` — zero getBoundingClientRect calls per frame.
    // Content changes (rows resizing as content-visibility renders them in,
    // windowed loading swapping children) mark the cache dirty; re-measuring
    // is throttled because during a fast scroll the observers fire every
    // frame and each measure forces a layout per anchor. A final measure at
    // scroll rest makes the settled tick authoritative.
    let offsets: number[] = [];
    let offsetsDirty = true;
    let lastMeasuredAt = Number.NEGATIVE_INFINITY;
    const MEASURE_THROTTLE_MS = 150;

    const refreshOffsets = () => {
      offsets = anchorOffsets(scroller, turns, anchors);
      offsetsDirty = false;
      lastMeasuredAt = performance.now();
    };

    const settle = () => {
      setScrolling(false);
      if (offsetsDirty) {
        refreshOffsets();
        setActive(currentTickFromOffsets(offsets, scroller.scrollTop));
      }
    };

    const update = () => {
      frame = 0;
      if (
        offsetsDirty &&
        performance.now() - lastMeasuredAt >= MEASURE_THROTTLE_MS
      ) {
        refreshOffsets();
      }
      setActive(currentTickFromOffsets(offsets, scroller.scrollTop));
      setScrolling(true);
      clearTimeout(quiet);
      quiet = setTimeout(settle, SCROLL_REST_MS);
    };

    const onScroll = () => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };

    // Invalidate when the content grows/shrinks (lazy Markdown, windowed
    // loading): on first open the bubbles mount asynchronously, so a single
    // mount-time measure finds no anchors and the strip points at the top
    // until the first scroll. The observer fires as rows stream in and the
    // active tick settles on the latest turn without user input.
    const invalidate = () => {
      offsetsDirty = true;
      if (frame === 0) frame = requestAnimationFrame(update);
    };
    const observer = new ResizeObserver(invalidate);
    for (const child of scroller.children) observer.observe(child);
    const childWatcher = new MutationObserver(() => {
      observer.disconnect();
      for (const child of scroller.children) observer.observe(child);
      invalidate();
    });
    childWatcher.observe(scroller, { childList: true });

    refreshOffsets();
    setActive(currentTickFromOffsets(offsets, scroller.scrollTop));
    scroller.addEventListener("scroll", onScroll, { passive: true });

    return () => {
      scroller.removeEventListener("scroll", onScroll);
      observer.disconnect();
      childWatcher.disconnect();
      if (frame !== 0) cancelAnimationFrame(frame);
      clearTimeout(quiet);
    };
  }, [turns, props.messagesRef]);

  async function jumpTo(turn: SessionTurnOutlineEntry) {
    setHovered(null);
    const revealed = await props.onRevealMessage(turn.message_index);
    if (!revealed) return;

    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const scroller = props.messagesRef;
        if (!scroller) return;
        anchorFor(scroller, turn.ordinal)?.scrollIntoView({
          behavior: "smooth",
          block: "start",
        });
      });
    });
  }

  function tickWidth(index: number): number {
    const hoveredIndex = hovered;
    const hoverDistance =
      hoveredIndex === null
        ? Number.POSITIVE_INFINITY
        : Math.abs(index - hoveredIndex);
    const scrollDistance = scrolling
      ? Math.abs(index - active)
      : Number.POSITIVE_INFINITY;
    return (
      MOUNTAIN[Math.min(hoverDistance, scrollDistance)]?.width ?? BASE_WIDTH
    );
  }

  function tickClass(index: number): string {
    if (index === active) return "timeline-minimap-tick-active";

    const hoveredIndex = hovered;
    const hoverDistance =
      hoveredIndex === null
        ? Number.POSITIVE_INFINITY
        : Math.abs(index - hoveredIndex);
    const scrollDistance = scrolling
      ? Math.abs(index - active)
      : Number.POSITIVE_INFINITY;
    return (
      MOUNTAIN[Math.min(hoverDistance, scrollDistance)]?.className ??
      "timeline-minimap-tick-base"
    );
  }

  function cardPosition(index: number): string {
    const count = turns.length;
    if (index < count / 3) return "timeline-minimap-card-top";
    if (index >= (count * 2) / 3) return "timeline-minimap-card-bottom";
    return "timeline-minimap-card-middle";
  }

  return turns.length >= MIN_TURNS_TO_SHOW ? (
    <div className="timeline-minimap">
      <div
        className="timeline-minimap-strip"
        onWheel={(event) => {
          props.messagesRef?.scrollBy({ top: event.deltaY });
        }}
      >
        {turns.map((turn, index) => (
          <div
            className="timeline-minimap-row"
            key={turn.ordinal}
            onMouseEnter={() => setHovered(index)}
            onMouseLeave={() => setHovered(null)}
          >
            <button
              type="button"
              aria-label={turn.user_text || `#${turn.ordinal + 1}`}
              className="timeline-minimap-button"
              onClick={() => void jumpTo(turn)}
            >
              <span
                className={`timeline-minimap-tick ${tickClass(index)}`}
                style={{ width: `${tickWidth(index)}px` }}
              />
            </button>
            {hovered === index && (
              <button
                type="button"
                className={`timeline-minimap-card ${cardPosition(index)}`}
                onClick={() => void jumpTo(turn)}
              >
                <span className="timeline-minimap-card-title">
                  {turn.user_text || "…"}
                </span>
                {turn.reply_text && (
                  <span className="timeline-minimap-card-reply">
                    {turn.reply_text}
                  </span>
                )}
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  ) : null;
}
