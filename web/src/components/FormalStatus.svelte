<script lang="ts">
  import type { FormalCodeStatus, LeanCertification, LeanRole } from "../lib/types";
  let { language, status, certification, role = null, compact = false }: {
    language: "Lean" | "Rocq"; status: FormalCodeStatus; certification?: LeanCertification | null;
    role?: LeanRole | null; compact?: boolean;
  } = $props();
  // A definition has nothing to prove: it reads as a definition until LeanGround registers it,
  // unless its submission went stale or was refused.
  const definition = $derived(role === "definition" &&
    !(certification && (certification.status === "stale" || certification.status === "rejected")));
  const labels = { no_code: "No code", unverified: "Unverified", verified: "Verified" };
  const certificationLabels = {
    not_submitted: "Not submitted to LeanGround", insufficient: "Insufficient evidence",
    derivable: "Derivable", certified: "Certified",
    stale: "Changed since submission", rejected: "Not accepted", registered: "Registered definition",
    imported: "Imported from upstream · covered by the project check",
  };
  // Two dimensions (docs/mathdoc.md §4.5): a local check is only an editing aid; certification comes
  // from LeanGround. The legacy backend reports only the local check.
  const text = $derived.by(() => {
    if (definition) return certification?.status === "registered" ? certificationLabels.registered : "Definition";
    if (certification === undefined) return labels[status];
    if (certification === null) return labels.no_code;
    const base = certificationLabels[certification.status];
    const truncated = certification.status === "insufficient" && certification.truncated ? " (search truncated)" : "";
    const reason = certification.status === "rejected" && certification.reason ? ` (${certification.reason})` : "";
    const local = status === "verified" ? " · local check passed" : "";
    return `${base}${truncated}${reason}${local}`;
  });
  const shown = $derived(definition ? (certification?.status === "registered" ? "registered" : "definition")
    : certification === undefined ? status : certification === null ? "no_code" : certification.status);
  const label = $derived(`${language}: ${text}`);
  // A refusal's explanation (Lean's messages, for a proof LeanGround rejected) shows on hover.
  const hover = $derived(certification?.status === "rejected" && certification.details ? `${label}\n\n${certification.details}`
    : certification?.status === "imported" ? `${label}\n\n${certification.path} at ${certification.commit.slice(0, 12)}` : label);
</script>

<span class="formal-status" data-status={shown} data-role={role === "definition" ? "definition" : undefined} title={hover} aria-label={label}>
  <span class="status-light" aria-hidden="true"></span>
  <span class="formal-language">{language}</span>
  {#if !compact}<span class="status-text">{text}</span>{/if}
</span>

<style>
  /* Verification reads as a status light plus a language, no surrounding pill. */
  .formal-status {
    display: inline-flex;
    align-items: center;
    gap: 0.34rem;
  }
  .status-light {
    width: 7px;
    height: 7px;
    flex: 0 0 auto;
    border-radius: 50%;
  }
  .formal-status[data-status="no_code"] .status-light {
    background: var(--mdc-muted);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-muted) 18%, transparent);
  }
  .formal-status[data-status="unverified"] .status-light {
    background: var(--mdc-warning);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-warning) 20%, transparent);
  }
  .formal-status[data-status="not_submitted"] .status-light, .formal-status[data-status="definition"] .status-light {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--mdc-muted);
  }
  /* A definition's light is a diamond, as in the graph: its kind shows without colour. */
  .formal-status[data-role="definition"] .status-light {
    width: 6px;
    height: 6px;
    margin-inline: 0.5px;
    border-radius: 1px;
    transform: rotate(45deg);
  }
  .formal-status[data-status="imported"] .status-light {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--mdc-accent);
  }
  .formal-status[data-status="insufficient"] .status-light {
    background: var(--mdc-warning);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-warning) 20%, transparent);
  }
  .formal-status[data-status="stale"] .status-light, .formal-status[data-status="rejected"] .status-light {
    background: var(--mdc-error);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-error) 20%, transparent);
  }
  .formal-status[data-status="derivable"] .status-light {
    background: var(--mdc-accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-accent) 22%, transparent);
  }
  .formal-status[data-status="verified"] .status-light, .formal-status[data-status="certified"] .status-light,
  .formal-status[data-status="registered"] .status-light {
    background: var(--mdc-accent-down);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-accent-down) 22%, transparent);
  }
  .formal-language {
    color: var(--mdc-fg-soft);
    font-weight: 600;
  }
  .status-text {
    color: var(--mdc-muted);
  }
</style>
