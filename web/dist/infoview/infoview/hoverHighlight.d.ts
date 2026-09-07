import React from 'react';
import { HoverState } from './tooltips';
export interface HoverHighlightSettings {
    ref: React.RefObject<HTMLSpanElement>;
    /**
     * Whether to return the `highlight` CSS class on hover.
     */
    highlightOnHover: boolean;
    /**
     * Whether to return the `underline` CSS class on hover while holding `Ctrl` / `Meta`.
     */
    underlineOnModHover: boolean;
}
export interface HoverHighlight {
    hoverState: HoverState;
    setHoverState: React.Dispatch<React.SetStateAction<HoverState>>;
    className: string;
    onPointerOver: (e: React.PointerEvent<HTMLSpanElement>) => void;
    onPointerOut: (e: React.PointerEvent<HTMLSpanElement>) => void;
    onPointerMove: (e: React.PointerEvent<HTMLSpanElement>) => void;
}
/**
 * Logic for a component that is highlighted/underlined when hovered over.
 * The component is passed in `settings.ref`.
 *
 * The hook returns the current hover state of the component,
 * a string of CSS classes containing `highlight` and/or `underline` when appropriate,
 * as well as event handlers which the the caller must attach to the component.
 */
export declare function useHoverHighlight(settings: HoverHighlightSettings): HoverHighlight;
