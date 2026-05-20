import { createEffect, onMount, onCleanup } from "solid-js";
import type { ProcessedEntry } from "./hooks";

const ROLE_COLORS: Record<string, string> = {
  user: "rgba(0, 122, 255, 0.7)",
  assistant: "rgba(156, 163, 175, 0.5)",
  tool: "rgba(16, 185, 129, 0.35)",
  system: "rgba(245, 158, 11, 0.6)",
};

const MERGED_TOOL_WEIGHT = 8;
const MIN_BLOCK_HEIGHT = 2;
const BLOCK_GAP = 1;
const MIN_ENTRIES_TO_SHOW = 10;
const MAX_BLOCKS = 600;

interface MinimapProps {
  entries: ProcessedEntry[];
  messagesRef: HTMLDivElement | undefined;
  onScrollToFraction?: (fraction: number) => void;
}

export function TimelineMinimap(props: MinimapProps) {
  let canvasRef: HTMLCanvasElement | undefined;
  let containerRef: HTMLDivElement | undefined;

  interface Block {
    y: number;
    h: number;
    color: string;
    entryIndex: number;
  }

  function computeBlocks(
    entries: ProcessedEntry[],
    canvasHeight: number,
  ): Block[] {
    const items: { color: string; weight: number; entryIndex: number }[] = [];
    const stride = Math.max(1, Math.ceil(entries.length / MAX_BLOCKS));
    for (let i = 0; i < entries.length; i += stride) {
      const e = entries[i];
      if (e.type === "time-sep") continue;
      if (e.type === "merged-tools") {
        items.push({
          color: ROLE_COLORS.tool,
          weight: MERGED_TOOL_WEIGHT,
          entryIndex: i,
        });
      } else {
        const weight = Math.max(1, e.msg.content?.length ?? 0);
        const color = ROLE_COLORS[e.msg.role] ?? ROLE_COLORS.assistant;
        items.push({ color, weight, entryIndex: i });
      }
    }
    if (items.length === 0) return [];

    const totalWeight = items.reduce((sum, it) => sum + it.weight, 0);
    const totalGaps = (items.length - 1) * BLOCK_GAP;
    const availableH = Math.max(0, canvasHeight - totalGaps);
    const blocks: Block[] = [];
    let y = 0;
    for (let idx = 0; idx < items.length; idx++) {
      const item = items[idx];
      const rawH = (item.weight / totalWeight) * availableH;
      const h = Math.max(MIN_BLOCK_HEIGHT, rawH);
      blocks.push({ y, h, color: item.color, entryIndex: item.entryIndex });
      y += h + BLOCK_GAP;
    }
    if (y > canvasHeight && blocks.length > 0) {
      const scale = canvasHeight / y;
      let accY = 0;
      for (const b of blocks) {
        b.y = accY;
        b.h = Math.max(1, b.h * scale);
        accY += b.h + BLOCK_GAP * scale;
      }
    }
    return blocks;
  }

  function drawBlocks(
    ctx: CanvasRenderingContext2D,
    blocks: Block[],
    width: number,
  ) {
    const dpr = window.devicePixelRatio || 1;
    ctx.clearRect(0, 0, width, ctx.canvas.height / dpr);
    const pad = 6;
    const barW = width - pad * 2;
    const radius = 2;
    for (const b of blocks) {
      ctx.fillStyle = b.color;
      ctx.beginPath();
      ctx.roundRect(pad, b.y, barW, Math.max(radius * 2, b.h), radius);
      ctx.fill();
    }
  }

  function drawViewport(
    ctx: CanvasRenderingContext2D,
    width: number,
    canvasHeight: number,
  ) {
    const el = props.messagesRef;
    if (!el) return;

    const { scrollTop, scrollHeight, clientHeight } = el;
    if (scrollHeight <= clientHeight) return;

    // Standard scroll: scrollTop=0 is top (oldest); scrollTop = max is
    // bottom (newest). Convert to a [0, 1] top-fraction so the indicator
    // tracks the top of the viewport on the minimap.
    const viewFraction = clientHeight / scrollHeight;
    const topFraction = scrollTop / (scrollHeight - clientHeight);

    const indicatorH = Math.max(8, viewFraction * canvasHeight);
    const indicatorY = topFraction * (canvasHeight - indicatorH);

    ctx.fillStyle = "rgba(255, 255, 255, 0.08)";
    ctx.beginPath();
    ctx.roundRect(2, indicatorY, width - 4, indicatorH, 3);
    ctx.fill();
    ctx.strokeStyle = "rgba(255, 255, 255, 0.2)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.roundRect(2.5, indicatorY + 0.5, width - 5, indicatorH - 1, 3);
    ctx.stroke();
  }

  let currentBlocks: Block[] = [];

  function repaint() {
    const canvas = canvasRef;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    ctx.scale(dpr, dpr);

    currentBlocks = computeBlocks(props.entries, rect.height);
    drawBlocks(ctx, currentBlocks, rect.width);
    drawViewport(ctx, rect.width, rect.height);
  }

  function repaintViewportOnly() {
    const canvas = canvasRef;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    drawBlocks(ctx, currentBlocks, rect.width);
    drawViewport(ctx, rect.width, rect.height);
  }

  function handleScroll() {
    repaintViewportOnly();
  }

  function fractionFromEvent(e: MouseEvent): number {
    const canvas = canvasRef;
    if (!canvas) return 0;
    const rect = canvas.getBoundingClientRect();
    return Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height));
  }

  function handleCanvasClick(e: MouseEvent) {
    props.onScrollToFraction?.(fractionFromEvent(e));
  }

  function handleCanvasMouseDown(e: MouseEvent) {
    e.preventDefault();

    let dragged = false;
    const onMove = (me: MouseEvent) => {
      dragged = true;
      props.onScrollToFraction?.(fractionFromEvent(me));
    };
    const onUp = (ue: MouseEvent) => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      if (!dragged) props.onScrollToFraction?.(fractionFromEvent(ue));
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  onMount(() => {
    const el = props.messagesRef;
    if (el) {
      el.addEventListener("scroll", handleScroll);
    }

    const ro = new ResizeObserver(() => repaint());
    if (containerRef) ro.observe(containerRef);

    onCleanup(() => {
      if (el) el.removeEventListener("scroll", handleScroll);
      ro.disconnect();
    });
  });

  createEffect(() => {
    const _entries = props.entries;
    repaint();
  });

  return (
    <div
      class="timeline-minimap"
      ref={containerRef}
      style={{
        display:
          props.entries.length < MIN_ENTRIES_TO_SHOW ? "none" : undefined,
      }}
    >
      <canvas
        ref={canvasRef}
        class="timeline-minimap-canvas"
        onMouseDown={handleCanvasMouseDown}
        onClick={handleCanvasClick}
      />
    </div>
  );
}
