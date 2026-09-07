import { HighlightedSubexprInfo, TaggedText } from '@leanprover/infoview-api';
export interface InteractiveTextComponentProps<T> {
    fmt: TaggedText<T>;
}
export interface InteractiveTagProps<T, U> extends InteractiveTextComponentProps<U> {
    tag: T;
}
export interface InteractiveTaggedTextProps<T, U> extends InteractiveTextComponentProps<U> {
    InnerTagUi: (_: InteractiveTagProps<T, U>) => JSX.Element;
}
/**
 * Core loop to display {@link TaggedText} objects. Invokes `InnerTagUi` on `tag` nodes in order to support
 * various embedded information, for example subexpression information stored in {@link CodeWithInfos}.
 */
export declare function InteractiveTaggedText<T, U>({ fmt, InnerTagUi }: InteractiveTaggedTextProps<T, U>): import("react/jsx-runtime").JSX.Element;
/**
 * Parse the `contents` as Markdown and render the result.
 *
 * This component applies some infoview-specific styling
 * and then passes the content through to a Markdown renderer
 * (currently `remark`).
 */
export declare function Markdown({ contents }: {
    contents: string;
}): JSX.Element;
export type InteractiveCodeProps = InteractiveTextComponentProps<HighlightedSubexprInfo>;
/** Displays a {@link CodeWithInfos} obtained via RPC from the Lean server. */
export declare function InteractiveCode(props: InteractiveCodeProps): import("react/jsx-runtime").JSX.Element;
