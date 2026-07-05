import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
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
  messagesRef: HTMLDivElement | undefined;
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
  const [active, setActive] = createSignal(0);
  const [hovered, setHovered] = createSignal<number | null>(null);
  const [scrolling, setScrolling] = createSignal(false);

  createEffect(() => {
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

    onCleanup(() => {
      scroller.removeEventListener("scroll", onScroll);
      if (frame !== 0) cancelAnimationFrame(frame);
      clearTimeout(quiet);
    });
  });

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
    const hoveredIndex = hovered();
    const hoverDistance =
      hoveredIndex === null
        ? Number.POSITIVE_INFINITY
        : Math.abs(index - hoveredIndex);
    const scrollDistance = scrolling()
      ? Math.abs(index - active())
      : Number.POSITIVE_INFINITY;
    return (
      MOUNTAIN[Math.min(hoverDistance, scrollDistance)]?.width ?? BASE_WIDTH
    );
  }

  function tickClass(index: number): string {
    if (index === active()) return "timeline-minimap-tick-active";

    const hoveredIndex = hovered();
    const hoverDistance =
      hoveredIndex === null
        ? Number.POSITIVE_INFINITY
        : Math.abs(index - hoveredIndex);
    const scrollDistance = scrolling()
      ? Math.abs(index - active())
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

  return (
    <Show when={props.outline.length >= MIN_TURNS_TO_SHOW}>
      <div class="timeline-minimap">
        <div
          class="timeline-minimap-strip"
          onWheel={(event) => {
            props.messagesRef?.scrollBy({ top: event.deltaY });
          }}
        >
          <For each={props.outline}>
            {(turn, index) => (
              <div
                class="timeline-minimap-row"
                onMouseEnter={() => setHovered(index())}
                onMouseLeave={() => setHovered(null)}
              >
                <button
                  type="button"
                  aria-label={turn.user_text || `#${turn.ordinal + 1}`}
                  class="timeline-minimap-button"
                  onClick={() => void jumpTo(turn)}
                >
                  <span
                    class={`timeline-minimap-tick ${tickClass(index())}`}
                    style={{ width: `${tickWidth(index())}px` }}
                  />
                </button>
                <Show when={hovered() === index()}>
                  <button
                    type="button"
                    class={`timeline-minimap-card ${cardPosition(index())}`}
                    onClick={() => void jumpTo(turn)}
                  >
                    <span class="timeline-minimap-card-title">
                      {turn.user_text || "…"}
                    </span>
                    <Show when={turn.reply_text}>
                      <span class="timeline-minimap-card-reply">
                        {turn.reply_text}
                      </span>
                    </Show>
                  </button>
                </Show>
              </div>
            )}
          </For>
        </div>
      </div>
    </Show>
  );
}
