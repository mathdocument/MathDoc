<script lang="ts">
  import type { FormalCodeStatus, LeanCertification } from "../lib/types";
  let { language, status, certification, compact = false }: {
    language: "Lean" | "Rocq"; status: FormalCodeStatus; certification?: LeanCertification | null; compact?: boolean;
  } = $props();
  const labels = { no_code: "No code", unverified: "Unverified", verified: "Verified" };
  const certificationLabels = {
    not_submitted: "Not submitted to LeanGround", insufficient: "Insufficient evidence",
    derivable: "Derivable", certified: "Certified",
  };
  // Two dimensions (plan §7.1): a local check is only an editing aid; certification comes
  // from LeanGround. The legacy backend reports only the local check.
  const text = $derived.by(() => {
    if (certification === undefined) return labels[status];
    if (certification === null) return labels.no_code;
    const base = certificationLabels[certification.status];
    const truncated = certification.status === "insufficient" && certification.truncated ? " (search truncated)" : "";
    const local = status === "verified" ? " · local check passed" : "";
    return `${base}${truncated}${local}`;
  });
  const shown = $derived(certification === undefined ? status : certification === null ? "no_code" : certification.status);
  const label = $derived(`${language}: ${text}`);
</script>

<span class="formal-status" data-status={shown} title={label} aria-label={label}>
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
  .formal-status[data-status="not_submitted"] .status-light {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--mdc-muted);
  }
  .formal-status[data-status="insufficient"] .status-light {
    background: var(--mdc-warning);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-warning) 20%, transparent);
  }
  .formal-status[data-status="derivable"] .status-light {
    background: var(--mdc-accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-accent) 22%, transparent);
  }
  .formal-status[data-status="verified"] .status-light, .formal-status[data-status="certified"] .status-light {
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
