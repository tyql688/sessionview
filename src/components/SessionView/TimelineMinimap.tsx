import { useEffect, useMemo, useState } from "react";
import type { SessionTurnOutlineEntry } from "../../lib/tauri";

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
 * space — recomputed per scroll frame because row heights shift. Works
 * unchanged for the column-reverse container: scrollTop and offsets are both
 * negative above the bottom, and only their ordering matters. Missing anchors
 * (turns outside the rendered window) come in as Infinity. */
function anchorOffsets(
  scroller: HTMLElement,
  turns: SessionTurnOutlineEntry[],
): number[] {
  const scrollerTop = scroller.getBoundingClientRect().top;
  return turns.map((turn) => {
    const anchor = anchorFor(scroller, turn.ordinal);
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

    const measure = () =>
      currentTickFromOffsets(
        anchorOffsets(scroller, turns),
        scroller.scrollTop,
      );

    const update = () => {
      frame = 0;
      setActive(measure());
      setScrolling(true);
      clearTimeout(quiet);
      quiet = setTimeout(() => setScrolling(false), SCROLL_REST_MS);
    };

    const onScroll = () => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };

    setActive(measure());
    scroller.addEventListener("scroll", onScroll, { passive: true });

    return () => {
      scroller.removeEventListener("scroll", onScroll);
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
