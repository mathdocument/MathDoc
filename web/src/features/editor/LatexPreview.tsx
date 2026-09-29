import { useEffect, useRef, type MouseEvent } from "react";
import { renderMath } from "../../lib/latex-math";
import { projectPath } from "../../lib/project-path";
import { chainEditorScroll } from "../../lib/editor-scroll";
import type { LatexLabel } from "../../lib/latex";

export function LatexPreview({
  html,
  fnode,
  labels,
  focusLabel,
  onNavigate,
}: {
  html: string;
  fnode: string;
  labels: LatexLabel[];
  focusLabel?: string;
  onNavigate?: (fnode: string, label: string) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  function scroll(id: string) {
    const target = document.getElementById(id);
    if (target && host.current?.contains(target))
      target.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }
  useEffect(() => {
    const action = chainEditorScroll(host.current!);
    return () => action.destroy();
  }, []);
  useEffect(() => {
    const target = host.current!;
    let alive = true;
    const clearMath = renderMath(target);
    for (const link of target.querySelectorAll<HTMLAnchorElement>(
      "a[data-latex-node]",
    ))
      link.href = `${projectPath("/")}#${new URLSearchParams({ ref: link.dataset.latexNode!, label: link.dataset.latexLabel! })}`;
    if (target.querySelector(".latex-diagram"))
      void import("../../lib/latex-diagram").then(({ renderDiagrams }) => {
        if (alive) renderDiagrams(target);
      });
    return () => {
      alive = false;
      clearMath();
    };
  }, [html]);
  useEffect(() => {
    const id = labels.find((label) => label.label === focusLabel)?.anchor;
    if (id) {
      const frame = requestAnimationFrame(() => scroll(id));
      return () => cancelAnimationFrame(frame);
    }
  }, [html, labels, focusLabel]);
  function navigate(event: MouseEvent) {
    if (
      event.button ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey ||
      event.shiftKey
    )
      return;
    const link = (event.target as Element).closest<HTMLAnchorElement>("a");
    if (!link || !host.current?.contains(link)) return;
    if (link.dataset.latexNode) {
      event.preventDefault();
      if (link.dataset.latexNode === fnode) {
        const id = labels.find(
          (label) => label.label === link.dataset.latexLabel,
        )?.anchor;
        if (id) scroll(id);
      } else onNavigate?.(link.dataset.latexNode, link.dataset.latexLabel!);
    } else if (link.getAttribute("href")?.startsWith("#")) {
      event.preventDefault();
      scroll(link.getAttribute("href")!.slice(1));
    }
  }
  // HTML continues to come from the existing escaped backend renderer.
  return (
    <div
      className="latex-preview"
      ref={host}
      onClick={navigate}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
