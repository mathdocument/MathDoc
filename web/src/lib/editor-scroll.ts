// Shared by source blocks and the same-origin Lean / Infoview frames.
const paneScroll = Symbol.for('mdc.paneScroll');
type ScrollPane = HTMLElement & { [paneScroll]?: ReturnType<typeof createPaneScroll> };

function containsWheelTarget(surface: Element, target: Node | null) {
  // WebKit also delivers a frame's wheel to its parent, targeting the iframe.
  for (let host: Element | null = surface; host; host = host.ownerDocument.defaultView?.frameElement ?? null) {
    if (host.contains(target)) return true;
  }
  return false;
}

function createPaneScroll(pane: ScrollPane) {
  const win = pane.ownerDocument.defaultView!;
  const surfaces = new Map<HTMLElement, string>();
  let active = false;
  let timer = 0;
  const release = () => {
    win.clearTimeout(timer);
    active = false;
    for (const [surface, overflow] of surfaces) surface.style.overflowY = overflow;
  };
  const handoff = () => {
    if (!active) {
      active = true;
      // Remove every inner scroller from the native chain for this gesture.
      // Keep hit testing intact: selection, links and buttons still work.
      for (const surface of surfaces.keys()) surface.style.overflowY = 'hidden';
    }
    win.clearTimeout(timer);
    // ponytail: DOM wheel events omit trackpad phases; idle ends the gesture.
    // This only restores inner scrolling, never delays or animates outer bounce.
    timer = win.setTimeout(release, 150);
  };
  const wheel = (event: WheelEvent) => {
    if (event.ctrlKey || event.shiftKey || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    if (active || ![...surfaces.keys()].some(surface => containsWheelTarget(surface, event.target as Node))) handoff();
  };
  pane.addEventListener('wheel', wheel, {passive: true});
  pane.addEventListener('scrollend', release);
  return {
    surfaces, handoff,
    get active() { return active; },
    destroy() {
      release();
      pane.removeEventListener('wheel', wheel);
      pane.removeEventListener('scrollend', release);
      delete pane[paneScroll];
    },
  };
}

/** Native inner scrolling without rubber-band; hand boundary gestures to the pane. */
export function chainEditorScroll(scroller: HTMLElement) {
  let host: Element | null = scroller;
  let pane: ScrollPane | null = null;
  while (host && !(pane = host.closest<ScrollPane>('.blocks'))) {
    host = host.ownerDocument.defaultView?.frameElement ?? null;
  }
  if (!pane) return {destroy() {}};
  const state = pane[paneScroll] ??= createPaneScroll(pane);
  const overscroll = scroller.style.overscrollBehaviorY;
  state.surfaces.set(scroller, scroller.style.overflowY);
  const updateBoundary = () => {
    const limit = scroller.scrollHeight - scroller.clientHeight;
    // At rest on an edge, let the very first wheel tick chain natively. Inside
    // the document, clamp rather than rubber-band before the next handoff.
    scroller.style.overscrollBehaviorY = scroller.scrollTop <= 1 || scroller.scrollTop >= limit - 1 ? 'auto' : 'none';
  };
  const resize = new ResizeObserver(updateBoundary);
  resize.observe(scroller);
  scroller.addEventListener('scroll', updateBoundary, {passive: true});
  updateBoundary();
  if (state.active) scroller.style.overflowY = 'hidden';
  const wheel = (event: WheelEvent) => {
    if (event.ctrlKey || event.shiftKey || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    const limit = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    const atEdge = event.deltaY > 0 ? scroller.scrollTop >= limit - 1 : scroller.scrollTop <= 1;
    if (state.active || atEdge) {
      state.handoff();
      // Flush before WebKit's native default action evaluates the scroll chain.
      getComputedStyle(scroller).overflowY;
    }
  };
  // A non-passive listener sends Safari down its synchronous scrolling path,
  // which cannot start rubber-banding a nested pane already at its boundary.
  scroller.addEventListener('wheel', wheel, {passive: true});
  const destroy = () => {
    resize.disconnect();
    scroller.removeEventListener('wheel', wheel);
    scroller.removeEventListener('scroll', updateBoundary);
    scroller.ownerDocument.defaultView?.removeEventListener('pagehide', destroy);
    scroller.style.overflowY = state.surfaces.get(scroller) ?? '';
    scroller.style.overscrollBehaviorY = overscroll;
    state.surfaces.delete(scroller);
    if (!state.surfaces.size) state.destroy();
  };
  scroller.ownerDocument.defaultView?.addEventListener('pagehide', destroy, {once: true});
  return {destroy};
}
