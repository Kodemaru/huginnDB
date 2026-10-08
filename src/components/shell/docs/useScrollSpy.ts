import * as React from "react";

/** How far below the pane's top edge a heading counts as "the one being read". */
const READING_LINE_PX = 96;

/**
 * Which of `ids` is being read in `scroller`: the last heading whose top has
 * crossed a line just under the pane's top edge.
 *
 * Driven by the pane's own `scroll` event, throttled to a frame, rather than an
 * `IntersectionObserver` — an observer reports *which headings are visible*,
 * and a short section shows three of them at once, so the "current" one would
 * be whichever fired last. A line to cross has one answer at any scroll offset.
 *
 * Two edges are handled on purpose. Before the first heading has crossed, the
 * first one is current (the reader is in its section even if its heading is
 * still below the line); and at the very bottom the last one is, because a short
 * final section can never scroll its heading up to the line.
 *
 * `scroller` is state rather than a ref because the viewer keys the page, so the
 * element is replaced whenever the page changes and the listener has to move
 * with it.
 */
export function useScrollSpy(
  scroller: HTMLElement | null,
  ids: readonly string[],
): string | null {
  const [active, setActive] = React.useState<string | null>(null);
  const key = ids.join("\n");

  React.useEffect(() => {
    if (!scroller || ids.length === 0) {
      setActive(null);
      return;
    }
    let frame = 0;
    const measure = () => {
      frame = 0;
      const line = scroller.getBoundingClientRect().top + READING_LINE_PX;
      let current = ids[0];
      for (const id of ids) {
        const el = document.getElementById(id);
        if (!el) continue;
        if (el.getBoundingClientRect().top <= line) current = id;
        else break;
      }
      const atBottom =
        scroller.scrollTop > 0 &&
        scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 4;
      setActive(atBottom ? ids[ids.length - 1] : current);
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(measure);
    };
    measure();
    scroller.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      scroller.removeEventListener("scroll", onScroll);
      if (frame) cancelAnimationFrame(frame);
    };
    // `key` stands for `ids`: a new array with the same headings is not a change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scroller, key]);

  return active;
}
