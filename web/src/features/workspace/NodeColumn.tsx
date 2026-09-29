import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useMemo,
  useCallback,
  type KeyboardEvent,
} from "react";
import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";
import { ArrowDownRight, ArrowUpRight, Search } from "lucide-react";
import type { NodePreview } from "../../lib/types";
import { shortFnode } from "../../lib/format";
import { FormalStatus } from "../../components/FormalStatus";

export function NodeColumn({
  items,
  title,
  accent,
  lastVisitedFnode,
  context,
  active,
  onSelect,
}: {
  items: NodePreview[];
  title: string;
  accent: "up" | "down";
  lastVisitedFnode: string | null;
  context: string | null;
  active: boolean;
  onSelect: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [focusIndex, setFocusIndex] = useState<number | null>(null);
  const list = useRef<HTMLUListElement>(null);
  const scrollPosition = useRef(0);
  const needle = query.trim().toLowerCase();
  const matches = useMemo(
    () =>
      needle
        ? items.filter((item) =>
            `${item.name} ${item.fnode}`.toLowerCase().includes(needle),
          )
        : items,
    [items, needle],
  );
  const measurements = useRef<VirtualItem[]>([]);
  const getItemKey = useCallback(
    (index: number) => matches[index]!.fnode,
    [matches],
  );
  const virtual = matches.length > 100;
  const virtualizer = useVirtualizer({
    count: matches.length,
    getScrollElement: () => list.current,
    estimateSize: () => 72,
    overscan: 5,
    getItemKey,
    initialMeasurementsCache: measurements.current,
    onChange: (instance) => {
      if (active) measurements.current = instance.takeSnapshot();
    },
    enabled: virtual && active,
    initialOffset: () => scrollPosition.current,
    measureElement: (element) => element.getBoundingClientRect().height,
  });
  const rows = virtual
    ? virtualizer.getVirtualItems()
    : matches.map((item, index) => ({ key: item.fnode, index, start: 0 }));
  useEffect(() => {
    setQuery("");
  }, [context]);
  useEffect(() => {
    scrollPosition.current = 0;
    if (list.current) list.current.scrollTop = 0;
  }, [context, needle]);
  useLayoutEffect(() => {
    if (!active) return;
    const offset = scrollPosition.current;
    let frame = requestAnimationFrame(() => {
      // Panel constraints settle first; restoring before their resize would let
      // measured rows adjust the scroll offset a second time.
      frame = requestAnimationFrame(() => {
        if (virtual) virtualizer.scrollToOffset(offset);
        if (list.current) list.current.scrollTop = offset;
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [active]);
  useLayoutEffect(() => {
    if (focusIndex === null) return;
    const button = list.current?.querySelector<HTMLButtonElement>(
      `button[data-index="${focusIndex}"]`,
    );
    if (button) {
      button.focus();
      setFocusIndex(null);
    }
  }, [rows, focusIndex]);
  async function moveFocus(event: KeyboardEvent, index: number) {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const target = (
      {
        ArrowDown: index + 1,
        ArrowUp: index - 1,
        Home: 0,
        End: matches.length - 1,
      } as Record<string, number>
    )[event.key];
    if (target === undefined) return;
    event.preventDefault();
    if (target < 0 || target >= matches.length) return;
    if (virtual) virtualizer.scrollToIndex(target, { align: "auto" });
    setFocusIndex(target);
  }
  return (
    <aside
      className={`column ${!active ? "hidden" : ""}`}
      data-accent={accent}
      aria-label={title}
    >
      <header className="column-head">
        <span>
          {accent === "up" ? (
            <ArrowUpRight size={15} />
          ) : (
            <ArrowDownRight size={15} />
          )}
          <strong>{title}</strong>
        </span>
        <span className="column-count">
          {needle ? `${matches.length}/${items.length}` : items.length}
        </span>
      </header>
      <p className="column-description">
        {accent === "up"
          ? "Ideas that build on this node"
          : "The foundations of this node"}
      </p>
      <label className="column-filter">
        <Search size={13} />
        <input
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          aria-label={`Filter ${title.toLowerCase()}`}
          placeholder="Filter by name or UUID..."
          spellCheck={false}
        />
      </label>
      <ul
        className={`cards ${virtual ? "virtual" : ""} ${matches.length ? "populated" : ""}`}
        ref={list}
        onScroll={() => {
          if (active && list.current?.clientHeight)
            scrollPosition.current = list.current.scrollTop;
        }}
      >
        {virtual && (
          <li
            className="spacer"
            aria-hidden="true"
            style={{ height: virtualizer.getTotalSize() }}
          />
        )}
        {rows.map((row) => {
          const item = matches[row.index]!;
          return (
            <li
              key={row.key}
              ref={virtual ? virtualizer.measureElement : undefined}
              data-index={row.index}
              data-fnode={item.fnode}
              aria-posinset={row.index + 1}
              aria-setsize={matches.length}
              style={
                virtual
                  ? {
                      position: "absolute",
                      top: 0,
                      width: "100%",
                      transform: `translateY(${row.start}px)`,
                    }
                  : undefined
              }
            >
              <button
                className={`card ${lastVisitedFnode === item.fnode ? "last-visited" : ""}`}
                data-fnode={item.fnode}
                data-index={row.index}
                aria-label={`${item.name} (${shortFnode(item.fnode)})`}
                onClick={() => onSelect(item.fnode)}
                onKeyDown={(event) => void moveFocus(event, row.index)}
              >
                <span className="card-title">{item.name}</span>
                <span className="card-meta">
                  <code>{shortFnode(item.fnode)}</code>
                  <span>d{item.depth}</span>
                  <span className="card-verification">
                    <FormalStatus
                      language="Lean"
                      status={item.formalization.lean}
                      compact
                    />
                    <FormalStatus
                      language="Rocq"
                      status={item.formalization.rocq}
                      compact
                    />
                  </span>
                </span>
              </button>
            </li>
          );
        })}
        {!matches.length && (
          <li className="column-empty">
            {needle ? "No matching nodes" : `No direct ${title.toLowerCase()}`}
          </li>
        )}
      </ul>
    </aside>
  );
}
