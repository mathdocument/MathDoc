import { useEffect, useRef, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { Braces, Plus } from "lucide-react";
import { api } from "../../lib/api";
import { errMsg } from "../../lib/format";
import { SOURCE_TYPES, type NodeDetail } from "../../lib/types";
import { trackMutation } from "../../lib/unsaved";
import { useLatest } from "../../hooks/use-latest";
import { Button } from "../../components/ui/button";
import { ErrorMessage } from "../../components/ui/dialog";
export function AddBlock({
  node,
  onAdded,
}: {
  node: NodeDetail;
  onAdded: (node: NodeDetail) => void;
}) {
  const [open, setOpen] = useState(false),
    [adding, setAdding] = useState<string | null>(null),
    [error, setError] = useState("");
  const latest = useLatest(node),
    alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const available = SOURCE_TYPES.filter(
    (type) => !node.blocks.some((block) => block.srctype === type),
  );
  async function add(srctype: string) {
    if (adding) return;
    const target = node;
    setAdding(srctype);
    setError("");
    const release = trackMutation();
    try {
      const updated = await api.putBlock(
        target.fnode,
        srctype,
        "",
        target.revision,
      );
      release();
      if (alive.current && latest.current.fnode === target.fnode) {
        setOpen(false);
        onAdded(updated);
      }
    } catch (error) {
      if (alive.current) setError(errMsg(error));
    } finally {
      release();
      if (alive.current) setAdding(null);
    }
  }
  return (
    <div className="add-block">
      <Menu.Root
        open={open}
        onOpenChange={(value) => {
          if (!adding) setOpen(value);
        }}
      >
        <Menu.Trigger
          render={
            <Button
              variant="quiet"
              className="add-btn"
              disabled={!available.length}
            />
          }
        >
          <Plus size={14} />
          Add source block
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="top" align="start" sideOffset={8}>
            <Menu.Popup className="menu-popup">
              {available.map((type) => (
                <Menu.Item
                  key={type}
                  className="menu-item"
                  disabled={!!adding}
                  closeOnClick={false}
                  onClick={() => void add(type)}
                >
                  <Braces size={14} />
                  {adding === type ? "adding..." : type}
                </Menu.Item>
              ))}
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      <ErrorMessage>{error}</ErrorMessage>
    </div>
  );
}
