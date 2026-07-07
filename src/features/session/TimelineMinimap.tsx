import { useState } from "react";
import { Button } from "@/components/ui/button";
import type { SessionTurnOutlineEntry } from "@/lib/tauri";

const MIN_TURNS_TO_SHOW = 2;

const MOUNTAIN = [
  { width: 24, className: "timeline-minimap-tick-peak" },
  { width: 18, className: "timeline-minimap-tick-near" },
  { width: 12, className: "timeline-minimap-tick-mid" },
  { width: 8, className: "timeline-minimap-tick-far" },
] as const;
const BASE_WIDTH = 5;

interface MinimapProps {
  /** Every turn of the session — one tick each; the strip compresses via
   * CSS, so no sampling and no cap. */
  outline: SessionTurnOutlineEntry[];
  /** Index into `outline` of the turn at the top of the viewport. Computed
   * by the owner from the virtualizer's rendered range — pure data, the
   * minimap itself never measures the DOM. */
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

export function TimelineMinimap(props: MinimapProps) {
  const turns = props.outline;
  const [hovered, setHovered] = useState<number | null>(null);

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
              onClick={() => {
                setHovered(null);
                void props.onRevealMessage(turn.message_index);
              }}
            >
              <span
                className={`timeline-minimap-tick ${tickClass(index)}`}
                style={{ width: `${tickWidth(index)}px` }}
              />
            </Button>
            {hovered === index && (
              <Button
                variant="ghost"
                type="button"
                className={`timeline-minimap-card h-auto items-stretch justify-start whitespace-normal active:translate-y-0 ${cardPosition(index)}`}
                onClick={() => {
                  setHovered(null);
                  void props.onRevealMessage(turn.message_index);
                }}
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
