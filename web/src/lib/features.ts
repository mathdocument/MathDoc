/**
 * The native Lean editor (lean.html with Infoview, backed by the legacy backend's local
 * Lean) exists only in builds made with MDC_WEB_LEAN_EDITOR=1. Otherwise Lean blocks are
 * ordinary source editors and certification comes from LeanGround (migration plan §7.1).
 */
export const localLeanEditor: boolean = __MDC_LEAN_EDITOR__;
