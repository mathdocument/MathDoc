import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Maximize2, Minus, Plus } from "lucide-react";
import type { FormalCodeStatus } from "../../lib/types";
import type { Theme } from "../../lib/theme";
import { useModel } from "../../hooks/use-model";
import { useLatest } from "../../hooks/use-latest";
import { Button, IconButton } from "../../components/ui/button";
import { GraphSession } from "./graph-session";
import { createGraphRenderer, type GraphRenderer } from "./renderer";
import "./graph.css";

export interface GraphHandle {
  prepare(): Promise<void>;
}
const statuses = ["unverified", "sorry", "conditional", "verified"] as const;
const labels = {
  unverified: "Unverified",
  sorry: "Sorry",
  conditional: "Conditional",
  verified: "Verified",
};
interface Props {
  active: boolean;
  theme: Theme;
  selectedFnode: string | null;
  revision: number;
  onSelect: (id: string | null) => void;
}
export default forwardRef<GraphHandle, Props>(function Graph(
  { active, theme, selectedFnode, revision, onSelect },
  ref,
) {
  const [session] = useState(() => new GraphSession());
  const [starredOnly, setStarredOnly] = useState(false);
  useModel(session);
  const host = useRef<HTMLDivElement>(null),
    canvas = useRef<HTMLCanvasElement>(null);
  const renderer = useRef<GraphRenderer | null>(null);
  const select = useLatest(onSelect);
  const shownNodes = useMemo(() => {
    const nodes = session.data?.nodes ?? [];
    return starredOnly ? nodes.filter((node) => node.starred) : nodes;
  }, [session.data, starredOnly]);
  const counts = useMemo(() => {
    const result: Record<FormalCodeStatus, number> = {
      unverified: 0,
      sorry: 0,
      conditional: 0,
      verified: 0,
    };
    for (const node of shownNodes)
      result[node.lean ?? "unverified"]++;
    return result;
  }, [shownNodes]);
  useLayoutEffect(() => {
    const instance = createGraphRenderer(canvas.current!, host.current!, (id) =>
      select.current(id),
    );
    renderer.current = instance;
    return () => {
      instance.destroy();
      renderer.current = null;
    };
  }, [select]);
  useLayoutEffect(() => {
    renderer.current?.update({ active, theme, selectedFnode, starredOnly });
  }, [active, theme, selectedFnode, starredOnly]);
  useLayoutEffect(() => {
    if (session.data) renderer.current?.setGraph(session.data);
  }, [session.data]);
  useEffect(() => {
    session.update(active, revision);
  }, [session, active, revision]);
  useEffect(() => () => session.cancel(), [session]);
  useImperativeHandle(ref, () => ({ prepare: () => session.prepare() }), [
    session,
  ]);
  const total = shownNodes.length;
  const selectionShown = selectedFnode && (!starredOnly || shownNodes.some((node) => node.fnode === selectedFnode));
  return (
    <div className="graph-container" ref={host}>
      <table className="graph-legend" aria-label="Lean verification colors">
        <tbody>
          {statuses.map((status) => (
            <tr key={status}>
              <th scope="row">
                <span className={`legend-label ${status}`}>
                  <i />
                  {labels[status]}
                </span>
              </th>
              <td>{counts[status].toLocaleString()}</td>
              <td className="count-divider">/</td>
              <td>{total.toLocaleString()}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {session.loading && !session.data && (
        <div className="graph-loading" role="status">
          <span className="spinner" />
          Loading graph
        </div>
      )}
      {starredOnly && session.data && !session.error && total === 0 && (
        <div className="graph-empty">No starred nodes. Use the star beside a node’s name.</div>
      )}
      {session.error && (
        <div className="graph-error" role="alert">
          {session.error}
          <Button onClick={() => void session.prepare()}>retry</Button>
        </div>
      )}
      <p className="graph-summary" role="status">
        {starredOnly ? "Starred graph" : "Knowledge graph"} with {total} {total === 1 ? "node" : "nodes"}.
        {selectionShown ? ` Selected node: ${selectedFnode}.` : ""} Use Search to
        select a node.
      </p>
      <canvas ref={canvas} aria-hidden="true" />
      <div className="graph-controls">
        <div className="zoom-controls" aria-label="zoom controls">
          <IconButton
            label="Zoom in"
            onClick={() => renderer.current?.zoom(1.3)}
          >
            <Plus size={15} />
          </IconButton>
          <IconButton
            label="Zoom out"
            onClick={() => renderer.current?.zoom(1 / 1.3)}
          >
            <Minus size={15} />
          </IconButton>
        </div>
        <div className="segmented" role="group" aria-label="Graph nodes">
          <button aria-pressed={!starredOnly} onClick={() => setStarredOnly(false)}>All nodes</button>
          <button aria-pressed={starredOnly} onClick={() => setStarredOnly(true)}>Starred</button>
        </div>
        <Button onClick={() => renderer.current?.fit()} aria-label="Fit graph">
          <Maximize2 size={13} />
          Fit graph
        </Button>
      </div>
    </div>
  );
});
