import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useMemo,
  useCallback,
  type KeyboardEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
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
  const pendingFocus = useRef<number | null>(null);
  const list = useRef<HTMLUListElement>(null);
  const column = useRef<HTMLElement>(null);
  const bounds = useRef({ width: 300, height: 600 });
  useLayoutEffect(() => {
    if (!active || !column.current) return;
    const element = column.current;
    const measure = () => {
      const { width, height } = element.getBoundingClientRect();
      if (width && height) bounds.current = { width, height };
    };
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    measure();
    return () => observer.disconnect();
  }, [active]);
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
    enabled: virtual,
    measureElement: (element) => element.getBoundingClientRect().height,
  });
  const rows = virtual
    ? virtualizer.getVirtualItems()
    : matches.map((item, index) => ({ key: item.fnode, index, start: 0 }));
  useEffect(() => {
    setQuery("");
  }, [context]);
  useEffect(() => {
    if (list.current) list.current.scrollTop = 0;
  }, [context, needle]);
  useLayoutEffect(() => {
    if (focusIndex === null) return;
    const button = list.current?.querySelector<HTMLButtonElement>(
      `button[data-index="${focusIndex}"]`,
    );
    if (button) {
      button.focus();
      pendingFocus.current = null;
      setFocusIndex(null);
    }
  }, [rows, focusIndex]);
  async function moveFocus(event: KeyboardEvent, index: number) {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const current = pendingFocus.current ?? index;
    const target = (
      {
        ArrowDown: current + 1,
        ArrowUp: current - 1,
        Home: 0,
        End: matches.length - 1,
      } as Record<string, number>
    )[event.key];
    if (target === undefined) return;
    event.preventDefault();
    if (target < 0 || target >= matches.length) return;
    if (virtual) virtualizer.scrollToIndex(target, { align: "auto" });
    const button = list.current?.querySelector<HTMLButtonElement>(
      `button[data-index="${target}"]`,
    );
    if (button) {
      pendingFocus.current = null;
      button.focus();
      setFocusIndex(null);
    } else {
      pendingFocus.current = target;
      setFocusIndex(target);
    }
  }
  return (
    <aside
      ref={column}
      className="column"
      aria-hidden={!active}
      // Retain the actual viewport while offscreen: a zero-size virtualizer
      // loses measured rows and shifts the user's scroll anchor on return.
      style={
        !active
          ? {
              position: "fixed",
              left: -10000,
              visibility: "hidden",
              ...bounds.current,
            }
          : undefined
      }
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
                  <code># {shortFnode(item.fnode)}</code>
                  <span>· Depth {item.depth}</span>
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
