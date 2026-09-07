import * as React from 'react';
export type TooltipProps = React.PropsWithChildren<React.HTMLProps<HTMLDivElement>> & {
    reference: HTMLElement | null;
};
export declare function Tooltip(props_: TooltipProps): React.ReactPortal;
export interface ToggleableTooltip {
    tooltip: JSX.Element;
    tooltipDisplayed: boolean;
    setTooltipDisplayed: (tooltipDisplayed: boolean) => void;
    onClick: () => void;
    onClickOutside: () => void;
}
/**
 * Provides handlers to show a tooltip when a state variable is changed.
 * The tooltip is hidden when a click is made anywhere (on or outside the tooltip).
 */
export declare function useToggleableTooltip(ref: React.RefObject<HTMLSpanElement>, tooltipChildren: React.ReactNode): ToggleableTooltip;
/** Hover state of an element. The pointer can be
 * - elsewhere (`off`)
 * - over the element (`over`)
 * - over the element with Ctrl or Meta (⌘ on Mac) held (`ctrlOver`)
 */
export type HoverState = 'off' | 'over' | 'ctrlOver';
export type TooltipState = 'pin' | 'show' | 'hide';
export interface HoverTooltip {
    state: TooltipState;
    onClick: (e: React.MouseEvent<HTMLSpanElement>) => void;
    onClickOutside: () => void;
    onMouseDown: (e: React.MouseEvent<HTMLSpanElement>) => void;
    onPointerDown: (e: React.PointerEvent<HTMLSpanElement>) => void;
    onPointerOver: (e: React.PointerEvent<HTMLSpanElement>) => void;
    onPointerOut: (e: React.PointerEvent<HTMLSpanElement>) => void;
    tooltip: JSX.Element;
}
/** Provides handlers to show a tooltip when the children of a component are hovered over or clicked.
 *
 * A `guardedOnClick` middleware can optionally be given in order to control what happens when the
 * hoverable area is clicked. The middleware can invoke `cont` to execute the default action,
 * which is to pin the tooltip open.
 */
export declare function useHoverTooltip(ref: React.RefObject<HTMLSpanElement>, tooltipChildren: React.ReactNode, guardedOnClick?: (e: React.MouseEvent<HTMLSpanElement, MouseEvent>, cont: (e: React.MouseEvent<HTMLSpanElement, MouseEvent>) => void) => void): HoverTooltip;
export type WithTooltipOnHoverProps = React.HTMLProps<HTMLSpanElement> & {
    tooltipChildren: React.ReactNode;
};
/**
 * Span that uses the logic of {@link useHoverTooltip}.
 */
export declare function WithTooltipOnHover(props_: WithTooltipOnHoverProps): import("react/jsx-runtime").JSX.Element;
