import { type CSSProperties, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import type { SessionTurnOutlineEntry } from "@/lib/tauri";
import { entryFirstMessageIndex, type ProcessedEntry } from "@/features/session/hooks";
import { atNewest, visualTopRow } from "@/features/session/timelineGeometry";

const MIN_TURNS_TO_SHOW = 2;

const MOUNTAIN = [
  { width: 24, className: "timeline-minimap-tick-peak" },
  { width: 18, className: "timeline-minimap-tick-near" },
  { width: 12, className: "timeline-minimap-tick-mid" },
  { width: 8, className: "timeline-minimap-tick-far" },
] as const;
const BASE_WIDTH = 5;
const MAX_TICK_WIDTH = MOUNTAIN[0].width;

interface MinimapProps {
  /** Every turn of the session — one tick each; the strip compresses via
   * CSS, so no sampling and no cap. */
  outline: SessionTurnOutlineEntry[];
  /** Index into `outline` of the turn at the top of the viewport — pure
   * data, the minimap itself never measures the DOM. */
  activeIndex: number;
  /** Whether the timeline is scrolling right now (drives the wave effect). */
  scrolling: boolean;
  /** Scroll the timeline by a wheel delta (the strip covers the scrollbar
   * edge, so wheel events over it should still move the messages). */
  onWheelScroll: (deltaY: number) => void;
  onRevealMessage: (messageIndex: number) => Promise<boolean>;
}

/** The turn the viewport is looking at: the LAST turn whose first message
 * sits at or above the viewport-top message. `null` means nothing measurable
 * is on screen yet (loading). */
export function activeTurnIndex(
  outline: SessionTurnOutlineEntry[],
  topMessageIndex: number | null,
  lastRowVisible: boolean,
): number {
  if (outline.length === 0) return 0;
  if (lastRowVisible) return outline.length - 1;
  if (topMessageIndex === null) return outline.length - 1;
  let current = 0;
  for (let i = 0; i < outline.length; i += 1) {
    if (outline[i].message_index <= topMessageIndex) {
      current = i;
    } else {
      break;
    }
  }
  return current;
}

/** How often the viewport-top hit-test may run during a scroll. The active
 * turn is a coarse indicator; 60Hz sampling would force layout and re-render
 * the minimap every frame for no visible gain. */
const HIT_TEST_INTERVAL_MS = 100;

/**
 * Owns every piece of scroll-driven minimap state (active turn, wave), fully
 * outside SessionView: it listens on the scroll container directly, so a
 * scroll frame re-renders at most this subtree — never the timeline rows or
 * the toolbar shell. The top-visible row is found by hit-testing the viewport
 * top (throttled), then mapped to a turn through `data-entry-key` → entries.
 */
export function TimelineMinimapDriver(props: {
  /** The timeline scroll container (null until it mounts). */
  scrollElement: HTMLDivElement | null;
  outline: SessionTurnOutlineEntry[];
  /** Role-filtered entries the timeline renders, for key → message mapping. */
  entries: ProcessedEntry[];
  onRevealMessage: (messageIndex: number) => Promise<boolean>;
}) {
  const [topVisibleKey, setTopVisibleKey] = useState<string | null>(null);
  const [lastRowVisible, setLastRowVisible] = useState(false);
  const [scrolling, setScrolling] = useState(false);

  useEffect(() => {
    const el = props.scrollElement;
    if (!el) return;
    let rafId = 0;
    let idleTimer = 0;
    let lastHitTest = 0;
    const sample = () => {
      const rect = el.getBoundingClientRect();
      // Hit-test a few points down from the top edge so the container's
      // top padding (which has no row under it) doesn't yield a null key —
      // otherwise the active turn falls back to the last turn at scrollTop 0.
      let row: Element | null = null;
      for (const dy of [6, 24, 48]) {
        const hit = document.elementFromPoint(rect.left + 24, rect.top + dy);
        row = hit instanceof Element ? hit.closest(".session-entry") : null;
        if (row) break;
      }
      // Still nothing (very top padding): the visually-top row.
      if (!row) row = visualTopRow(el);
      setTopVisibleKey(row?.getAttribute("data-entry-key") ?? null);
      setLastRowVisible(atNewest(el));
    };
    const onScroll = () => {
      if (rafId) return;
      rafId = requestAnimationFrame(() => {
        rafId = 0;
        setScrolling(true);
        window.clearTimeout(idleTimer);
        // The idle callback re-samples so a fast flick that ends inside the
        // throttle window still lands the active tick on its final position.
        idleTimer = window.setTimeout(() => {
          setScrolling(false);
          sample();
        }, 160);
        const now = performance.now();
        if (now - lastHitTest < HIT_TEST_INTERVAL_MS) return;
        lastHitTest = now;
        sample();
      });
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(rafId);
      window.clearTimeout(idleTimer);
    };
  }, [props.scrollElement]);

  // Key → entry-index map, built lazily and cached by entries identity: a
  // pagination chunk changes `entries` many times between scroll samples, and
  // an eager useMemo would rebuild the map on every chunk for nothing.
  const entriesRef = useRef(props.entries);
  entriesRef.current = props.entries;
  const indexCacheRef = useRef<{ entries: ProcessedEntry[]; map: Map<string, number> } | null>(null);
  // A key's message index never changes (keys embed absolute indices), so the
  // sampled key alone determines the result — entries updates don't need to
  // recompute it.
  const topVisibleMessageIndex = useMemo(() => {
    if (!topVisibleKey) return null;
    const entries = entriesRef.current;
    if (indexCacheRef.current?.entries !== entries) {
      indexCacheRef.current = { entries, map: new Map(entries.map((entry, index) => [entry.key, index])) };
    }
    // A stale key (role-filter change before the next sample) falls back to
    // scanning from the first entry.
    const start = indexCacheRef.current.map.get(topVisibleKey) ?? 0;
    for (let i = start; i < entries.length; i += 1) {
      const index = entryFirstMessageIndex(entries[i]);
      if (index !== null) return index;
    }
    return null;
  }, [topVisibleKey]);
  const activeIndex = activeTurnIndex(props.outline, topVisibleMessageIndex, lastRowVisible);

  const scrollElementRef = useRef(props.scrollElement);
  scrollElementRef.current = props.scrollElement;

  return (
    <TimelineMinimap
      outline={props.outline}
      activeIndex={activeIndex}
      scrolling={scrolling}
      onWheelScroll={(deltaY) => {
        scrollElementRef.current?.scrollBy({ top: deltaY });
      }}
      onRevealMessage={props.onRevealMessage}
    />
  );
}

export function TimelineMinimap(props: MinimapProps) {
  const turns = props.outline;
  const [hovered, setHovered] = useState<number | null>(null);

  // Every tick — the last one included — reveals its turn's QUESTION at the
  // viewport top (the hover card shows exactly that question). When a turn
  // sits too close to the end for a full viewport below it, scrollIntoView
  // clamps to the bottom natively; recenterWindowAround end-aligns the loaded
  // window near the tail so that clamp is computed against the real bottom.
  function revealTurn(index: number): void {
    setHovered(null);
    void props.onRevealMessage(turns[index].message_index);
  }

  function tickWidth(index: number): number {
    const hoveredIndex = hovered;
    const hoverDistance = hoveredIndex === null ? Number.POSITIVE_INFINITY : Math.abs(index - hoveredIndex);
    const scrollDistance = props.scrolling ? Math.abs(index - props.activeIndex) : Number.POSITIVE_INFINITY;
    return MOUNTAIN[Math.min(hoverDistance, scrollDistance)]?.width ?? BASE_WIDTH;
  }

  function tickClass(index: number): string {
    if (index === props.activeIndex) return "timeline-minimap-tick-active";

    const hoveredIndex = hovered;
    const hoverDistance = hoveredIndex === null ? Number.POSITIVE_INFINITY : Math.abs(index - hoveredIndex);
    const scrollDistance = props.scrolling ? Math.abs(index - props.activeIndex) : Number.POSITIVE_INFINITY;
    return MOUNTAIN[Math.min(hoverDistance, scrollDistance)]?.className ?? "timeline-minimap-tick-base";
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
          props.onWheelScroll(event.deltaY);
        }}
      >
        {turns.map((turn, index) => (
          <div
            className="timeline-minimap-row"
            key={turn.ordinal}
            onMouseEnter={() => setHovered(index)}
            onMouseLeave={() => setHovered(null)}
          >
            <Button
              variant="ghost"
              type="button"
              aria-label={turn.user_text || `#${turn.ordinal + 1}`}
              className="timeline-minimap-button h-auto min-h-0 rounded-none active:translate-y-0"
              onClick={() => revealTurn(index)}
            >
              <span
                className={`timeline-minimap-tick ${tickClass(index)}`}
                style={{ "--tick-scale": tickWidth(index) / MAX_TICK_WIDTH } as CSSProperties}
              />
            </Button>
            {hovered === index && (
              <Button
                variant="ghost"
                type="button"
                className={`timeline-minimap-card h-auto items-stretch justify-start whitespace-normal active:translate-y-0 ${cardPosition(index)}`}
                onClick={() => revealTurn(index)}
              >
                <span className="timeline-minimap-card-title">{turn.user_text || "…"}</span>
                {turn.reply_text && <span className="timeline-minimap-card-reply">{turn.reply_text}</span>}
              </Button>
            )}
          </div>
        ))}
      </div>
    </div>
  ) : null;
}
