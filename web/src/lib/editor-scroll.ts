/** With overscroll-behavior-y:none, hand excess wheel movement to the node pane. */
export function chainEditorScroll(scroller: HTMLElement) {
  const wheel = (event: WheelEvent) => {
    if (event.defaultPrevented || event.ctrlKey || event.shiftKey ||
        Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    const pane = scroller.closest<HTMLElement>('.blocks');
    if (!pane) return;
    const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? parseFloat(getComputedStyle(scroller).lineHeight) || 16
      : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? scroller.clientHeight : 1;
    const delta = event.deltaY * unit;
    const limit = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    const current = Math.max(0, Math.min(limit, scroller.scrollTop));
    const next = Math.max(0, Math.min(limit, current + delta));
    const excess = delta - (next - current);
    if (!excess) return; // Keep native scrolling inside the editor.
    event.preventDefault();
    scroller.scrollTop = next;
    pane.scrollBy({top: excess, behavior: 'instant'});
  };
  scroller.addEventListener('wheel', wheel, {passive: false});
  return {destroy: () => scroller.removeEventListener('wheel', wheel)};
}
