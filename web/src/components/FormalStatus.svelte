<script lang="ts">
  import type { FormalCodeStatus } from "../lib/types";
  let { language, status, compact = false }: {
    language: "Lean" | "Rocq"; status: FormalCodeStatus; compact?: boolean;
  } = $props();
  const labels = { no_code: "No code", unverified: "Unverified", verified: "Verified" };
  const label = $derived(`${language}: ${labels[status]}`);
</script>

<span class="formal-status" data-status={status} title={label} aria-label={label}>
  <span class="status-light" aria-hidden="true"></span>
  <span class="formal-language">{language}</span>
  {#if !compact}<span class="status-text">{labels[status]}</span>{/if}
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
  .formal-status[data-status="verified"] .status-light {
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
