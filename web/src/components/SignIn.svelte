<script lang="ts">
  import { KeyRound } from "@lucide/svelte";
  import { setAccessToken } from "../lib/auth";

  let token = $state("");
  let busy = $state(false);
  let error = $state("");
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || !token.trim()) return;
    busy = true; error = "";
    try {
      const response = await fetch("/api/me", { headers: { authorization: `Bearer ${token.trim()}` } });
      if (!response.ok) throw new Error(response.status === 401 ? "This access token is not valid." : `HTTP ${response.status}`);
      setAccessToken(token.trim());
      location.reload();
    } catch (e) { error = e instanceof Error ? e.message : String(e); busy = false; }
  }
</script>

<main class="sign-in">
  <form onsubmit={submit} aria-label="Sign in">
    <img src="/mdc-logo.svg" alt="" />
    <h1>Sign in to MathDoc</h1>
    <label>Access token<input type="password" autocomplete="current-password" bind:value={token} disabled={busy} /></label>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <button type="submit" disabled={busy || !token.trim()}><KeyRound size={15} />Sign in</button>
  </form>
</main>

<style>
  .sign-in { height: 100%; display: grid; place-items: center; padding: 24px; }
  form { display: grid; gap: 14px; width: min(360px, 100%); padding: 28px; border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-md); background: var(--mdc-panel); }
  img { width: 34px; }
  h1 { margin: 0; font-size: 20px; font-weight: 600; }
  label { display: grid; gap: 6px; font-size: 12px; color: var(--mdc-dim); }
  input { height: 38px; padding: 0 10px; border: 1px solid var(--mdc-border-strong); border-radius: var(--mdc-radius-sm); background: var(--mdc-bg); color: var(--mdc-fg); }
  button { display: inline-flex; gap: 8px; align-items: center; justify-content: center; height: 36px; border: 1px solid var(--mdc-accent); border-radius: var(--mdc-radius-sm); background: var(--mdc-accent); color: white; cursor: pointer; }
  button:disabled { opacity: .5; cursor: default; }
  .error { margin: 0; color: var(--mdc-error); font-size: 12px; }
</style>
