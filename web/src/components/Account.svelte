<script lang="ts">
  // Who this browser is signed in as; signing out forgets the access token and shows the sign-in page.
  import { onMount } from "svelte";
  import { LogOut } from "@lucide/svelte";
  import { signOut } from "../lib/auth";
  import { coordination } from "../lib/coordination";

  let actor = $state<string | null>(null);
  onMount(() => {
    coordination.me().then(me => { actor = me.actor; }, () => {});
  });
</script>

{#if actor}<span class="account" title={`Signed in as ${actor}`}>{actor}</span>{/if}
<button class="sign-out" onclick={signOut} title="Sign out" aria-label="Sign out"><LogOut size={15} strokeWidth={1.8} /></button>

<style>
  .account {
    max-width: 12rem;
    overflow: hidden;
    color: var(--mdc-dim);
    font-size: var(--mdc-text-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sign-out {
    display: grid;
    place-items: center;
    flex: 0 0 32px;
    width: 32px;
    height: 32px;
    padding: 0;
    color: var(--mdc-dim);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
  }
  .sign-out:hover { background: var(--mdc-card-hover); color: var(--mdc-fg); }
</style>
