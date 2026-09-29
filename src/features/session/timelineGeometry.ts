/**
 * Coordinate helpers for the column-reverse timeline scroller.
 *
 * The scroller anchors to the BOTTOM: scrollTop is 0 at the newest message
 * and negative upward; the oldest row sits at -(scrollHeight - clientHeight).
 * DOM rows are newest-first while entry arrays stay oldest-first — every
 * translation between the two lives here so the convention has one home.
 */

/** Pixels between the viewport top and the oldest loaded content. */
export function distanceToOldest(el: HTMLElement): number {
  return el.scrollHeight - el.clientHeight + el.scrollTop;
}

/** Pixels between the viewport bottom and the newest loaded content. */
export function distanceToNewest(el: HTMLElement): number {
  return -el.scrollTop;
}

/** Whether the newest message is (effectively) on screen. */
export function atNewest(el: HTMLElement): boolean {
  return el.scrollTop > -4;
}

/** Viewport bottom edge in the rows' offsetTop space. Its origin is the
 * scroller's padding-box top at scrollTop 0 (the newest end); older rows sit at
 * negative offsets, so a row's viewport position is `offsetTop - scrollTop`. */
export function viewportBottom(el: HTMLElement): number {
  return el.clientHeight + el.scrollTop;
}

/** Rubber-band overscroll past the oldest edge. The scrollable range is
 * clamped: content shorter than the viewport has no range and never reads
 * as overscrolled. */
export function overscrolledTop(el: HTMLElement): boolean {
  return el.scrollTop < -Math.max(0, el.scrollHeight - el.clientHeight);
}

/** Rubber-band overscroll past the newest edge. */
export function overscrolledBottom(el: HTMLElement): boolean {
  return el.scrollTop > 0;
}

/** All timeline rows, newest-first (DOM order). Rows sit inside
 * display:contents block wrappers, so never walk scroller children. */
function timelineRows(el: HTMLElement): NodeListOf<Element> {
  return el.querySelectorAll(".session-entry");
}

/** Row for an oldest-first entry index. */
export function rowAtEntryIndex(el: HTMLElement, index: number): Element | null {
  const rows = timelineRows(el);
  return rows.item(rows.length - 1 - index);
}

/** The visually top-most row (DOM last). */
export function visualTopRow(el: HTMLElement): Element | null {
  const rows = timelineRows(el);
  return rows.item(rows.length - 1);
}

/**
 * Poll once per animation frame until `settled()` holds for `frames`
 * consecutive frames or `timeoutMs` elapses. The predicate may keep closure
 * state (e.g. compare scrollTop across frames to detect a finished bounce).
 * Used to wait out WKWebView rubber-band animations, which override
 * programmatic scrollTop writes while running.
 */
export async function settleFrames(settled: () => boolean, frames: number, timeoutMs: number): Promise<void> {
  const start = performance.now();
  let stable = 0;
  while (stable < frames && performance.now() - start < timeoutMs) {
    await new Promise(requestAnimationFrame);
    stable = settled() ? stable + 1 : 0;
  }
}
