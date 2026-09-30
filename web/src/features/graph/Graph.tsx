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
  useModel(session);
  const host = useRef<HTMLDivElement>(null),
    canvas = useRef<HTMLCanvasElement>(null);
  const renderer = useRef<GraphRenderer | null>(null);
  const select = useLatest(onSelect);
  const counts = useMemo(() => {
    const result: Record<FormalCodeStatus, number> = {
      unverified: 0,
      sorry: 0,
      conditional: 0,
      verified: 0,
    };
    for (const node of session.data?.nodes ?? [])
      result[node.lean ?? "unverified"]++;
    return result;
  }, [session.data]);
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
    renderer.current?.update({ active, theme, selectedFnode });
  }, [active, theme, selectedFnode]);
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
  const total = session.data?.nodes.length ?? 0;
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
      {session.error && (
        <div className="graph-error" role="alert">
          {session.error}
          <Button onClick={() => void session.prepare()}>retry</Button>
        </div>
      )}
      <p className="graph-summary" role="status">
        Knowledge graph with {total} nodes.
        {selectedFnode ? ` Selected node: ${selectedFnode}.` : ""} Use Search to
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
        <Button onClick={() => renderer.current?.fit()} aria-label="Fit graph">
          <Maximize2 size={13} />
          Fit graph
        </Button>
      </div>
    </div>
  );
});
