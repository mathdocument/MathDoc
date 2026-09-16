/** Clamp editor wheel scrolling; leave boundary events to the native outer scroller. */
export function chainEditorScroll(scroller: HTMLElement) {
  const overflow = scroller.style.overflowY;
  let frame = 0;
  const restore = () => {
    cancelAnimationFrame(frame);
    frame = 0;
    scroller.style.overflowY = overflow;
  };
  const wheel = (event: WheelEvent) => {
    if (frame) restore();
    if (event.defaultPrevented || event.ctrlKey || event.shiftKey ||
        Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? parseFloat(getComputedStyle(scroller).lineHeight) || 16
      : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? scroller.clientHeight : 1;
    const delta = event.deltaY * unit;
    const limit = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    const current = Math.max(0, Math.min(limit, scroller.scrollTop));
    const atEdge = delta > 0 ? current >= limit - 1 : current <= 1;
    if (!atEdge) {
      // Like Monaco, consume movement only while the document can scroll.
      event.preventDefault();
      scroller.scrollTop = Math.max(0, Math.min(limit, current + delta));
      return;
    }
    // Safari would rubber-band this native editor before chaining. Skip it for
    // this default action, then restore its scrollbar before the next paint.
    // Recalculate the style now: deferring it also defers WebKit's handoff.
    scroller.style.overflowY = 'hidden';
    getComputedStyle(scroller).overflowY;
    frame = requestAnimationFrame(restore);
  };
  scroller.addEventListener('wheel', wheel, {passive: false});
  return {destroy() { scroller.removeEventListener('wheel', wheel); restore(); }};
}
