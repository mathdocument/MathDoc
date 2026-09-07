import { FVarId, MVarId, SubexprPos } from '@leanprover/infoview-api';
import * as React from 'react';
/**
 * A location within a goal. It is either:
 * - one of the hypotheses; or
 * - (a subexpression of) the type of one of the hypotheses; or
 * - (a subexpression of) the value of one of the let-bound hypotheses; or
 * - (a subexpression of) the goal type. */
export type GoalLocation = {
    hyp: FVarId;
} | {
    hypType: [FVarId, SubexprPos];
} | {
    hypValue: [FVarId, SubexprPos];
} | {
    target: SubexprPos;
};
export declare namespace GoalLocation {
    function isEqual(l1: GoalLocation, l2: GoalLocation): boolean;
    function withSubexprPos(l: GoalLocation, p: SubexprPos): GoalLocation;
}
/**
 * A location within a goal state. It identifies a specific goal together with a {@link GoalLocation}
 * within it.  */
export interface GoalsLocation {
    /** Which goal the location is in. */
    mvarId: MVarId;
    loc: GoalLocation;
}
export declare namespace GoalsLocation {
    function isEqual(l1: GoalsLocation, l2: GoalsLocation): boolean;
    function withSubexprPos(l: GoalsLocation, p: SubexprPos): GoalsLocation;
}
/**
 * An interface available through a React context in components
 * where selecting subexpressions makes sense.
 * Currently, subexpressions can only be selected in the tactic state.
 * We use {@link GoalsLocation} to represent subexpression positions in a goal.
 */
export interface Locations {
    isSelected: (l: GoalsLocation) => boolean;
    setSelected: (l: GoalsLocation, fn: React.SetStateAction<boolean>) => void;
    /**
     * A template for the location of the current component.
     * It is defined if and only if the current component is a subexpression of a selectable expression.
     * We use {@link GoalsLocation.withSubexprPos} to map this template to a complete location.
     */
    subexprTemplate?: GoalsLocation;
}
export declare const LocationsContext: React.Context<Locations | undefined>;
export type SelectableLocationSettings = {
    isSelectable: true;
    /** The {@link GoalsLocation} that can be selected by interacting with the span. */
    loc: GoalsLocation;
} | {
    isSelectable: false;
};
export interface SelectableLocation {
    className: string;
    /** Returns whether propagation of the click event within the same handler should be stopped. */
    onClick: (e: React.MouseEvent<HTMLSpanElement, MouseEvent>) => boolean;
    onPointerDown: (e: React.PointerEvent<HTMLSpanElement>) => void;
    /** An object that should be spliced into `data-vscode-context`. */
    dataVscodeContext: object;
}
/**
 * Logic for a component that can be selected using Shift-click and is highlighted when selected.
 *
 * The hook returns
 * - a string of CSS classes containing `highlight-selected` when appropriate; and
 * - event handlers which the the caller must attach to the component; and
 * - an object to append to `data-vscode-context`
 *   in order to display context menu entries to (un)select this location in VSCode.
 */
export declare function useSelectableLocation(settings: SelectableLocationSettings): SelectableLocation;
