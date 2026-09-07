import * as React from 'react';
import { InteractiveGoal, InteractiveTermGoal, RpcSessionAtPos, UserWidgetInstance } from '@leanprover/infoview-api';
import { GoalsLocation } from './goalLocation';
import { DocumentPosition } from './util';
/**
 * Fetch source code from Lean and dynamically import it as a JS module.
 *
 * The source must hash to `hash` (in Lean)
 * and must have been annotated with `@[widget]` or `@[widget_module]`
 * at some point before `pos`.
 *
 * If `hash` does not correspond to a registered module,
 * the promise is rejected with an error.
 *
 * #### Experimental `import` support for widget modules
 *
 * The module may import other `@[widget_module]`s by hash
 * using the URI scheme `'widget_module:hash,<hash>'`
 * where `<hash>` is a decimal representation
 * of the hash stored in `Lean.Widget.Module.javascriptHash`.
 *
 * In the future,
 * we may support importing widget modules by their fully qualified Lean name
 * (e.g. `'widget_module:name,Lean.Meta.Tactic.TryThis.tryThisWidget'`),
 * or some way to assign widget modules a more NPM-friendly name
 * so that the usual URIs (e.g. `'@leanprover-community/pro-widgets'`) work.
 */
export declare function importWidgetModule(rs: RpcSessionAtPos, pos: DocumentPosition, hash: string): Promise<any>;
export interface DynamicComponentProps {
    hash: string;
    props: any;
    /** @deprecated set {@link EnvPosContext} instead */
    pos?: DocumentPosition;
}
/**
 * Use {@link importWidgetModule} to import a module
 * which must `export default` a React component,
 * and render that with `props`.
 * Errors in the component are caught in an error boundary.
 *
 * The {@link EnvPosContext} must be set.
 * It is used to retrieve the `Lean.Environment`
 * from which the widget module identified by `hash`
 * is obtained.
 */
export declare function DynamicComponent(props_: React.PropsWithChildren<DynamicComponentProps>): import("react/jsx-runtime").JSX.Element;
interface PanelWidgetDisplayProps {
    pos: DocumentPosition;
    goals: InteractiveGoal[];
    termGoal?: InteractiveTermGoal;
    selectedLocations: GoalsLocation[];
    widget: UserWidgetInstance;
}
/** Props that every infoview panel widget receives as input to its `default` export. */
export interface PanelWidgetProps {
    /** Cursor position in the file at which the widget is being displayed. */
    pos: DocumentPosition;
    /** The current tactic-mode goals. */
    goals: InteractiveGoal[];
    /** The current term-mode goal, if any. */
    termGoal?: InteractiveTermGoal;
    /** Locations currently selected in the goal state. */
    selectedLocations: GoalsLocation[];
}
export declare function PanelWidgetDisplay({ pos, goals, termGoal, selectedLocations, widget }: PanelWidgetDisplayProps): import("react/jsx-runtime").JSX.Element;
export {};
