import { useEffect, useState } from "react";
import type { SessionTurnOutlineEntry } from "../../lib/tauri";

const MIN_TURNS_TO_SHOW = 2;
const SCROLL_REST_MS = 180;
const ACTIVE_MARGIN_PX = 32;

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

function currentTickFromScroll(
  scroller: HTMLElement,
  turns: SessionTurnOutlineEntry[],
): number {
  if (turns.length === 0) return 0;

  const scrollerRect = scroller.getBoundingClientRect();
  let closestVisible = -1;
  let closestDistance = Number.POSITIVE_INFINITY;

  for (let i = 0; i < turns.length; i += 1) {
    const anchor = anchorFor(scroller, turns[i].ordinal);
    if (!anchor) continue;

    const distance = Math.abs(
      anchor.getBoundingClientRect().top - scrollerRect.top - ACTIVE_MARGIN_PX,
    );
    if (distance < closestDistance) {
      closestDistance = distance;
      closestVisible = i;
    }
  }

  if (closestVisible >= 0) return closestVisible;

  const scrollRange = scroller.scrollHeight - scroller.clientHeight;
  if (scrollRange <= 0) return turns.length - 1;

  const bottomFraction = Math.max(
    0,
    Math.min(1, -scroller.scrollTop / scrollRange),
  );
  return Math.round((1 - bottomFraction) * (turns.length - 1));
}

export function TimelineMinimap(props: MinimapProps) {
  const [active, setActive] = useState(0);
  const [hovered, setHovered] = useState<number | null>(null);
  const [scrolling, setScrolling] = useState(false);

  useEffect(() => {
    const currentTurns = props.outline;
    const scroller = props.messagesRef;
    if (!scroller || currentTurns.length < MIN_TURNS_TO_SHOW) return;

    let frame = 0;
    let quiet: ReturnType<typeof setTimeout> | undefined;

    const update = () => {
      frame = 0;
      setActive(currentTickFromScroll(scroller, currentTurns));
      setScrolling(true);
      clearTimeout(quiet);
      quiet = setTimeout(() => setScrolling(false), SCROLL_REST_MS);
    };

    const onScroll = () => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };

    setActive(currentTickFromScroll(scroller, currentTurns));
    scroller.addEventListener("scroll", onScroll, { passive: true });

    return () => {
      scroller.removeEventListener("scroll", onScroll);
      if (frame !== 0) cancelAnimationFrame(frame);
      clearTimeout(quiet);
    };
  }, [props.outline, props.messagesRef]);

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
    const count = props.outline.length;
    if (index < count / 3) return "timeline-minimap-card-top";
    if (index >= (count * 2) / 3) return "timeline-minimap-card-bottom";
    return "timeline-minimap-card-middle";
  }

  return props.outline.length >= MIN_TURNS_TO_SHOW ? (
    <div className="timeline-minimap">
      <div
        className="timeline-minimap-strip"
        onWheel={(event) => {
          props.messagesRef?.scrollBy({ top: event.deltaY });
        }}
      >
        {props.outline.map((turn, index) => (
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
