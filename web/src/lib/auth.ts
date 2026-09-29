// Access tokens identify the author of every request (plan §6.3). The token stays in
// this browser profile; a 401 response clears it and returns to the sign-in page.
const KEY = "mdc-access-token";

export function accessToken(): string | null {
  try { return localStorage.getItem(KEY); } catch { return null; }
}
export function setAccessToken(token: string) {
  localStorage.setItem(KEY, token);
}
export function signOut() {
  try { localStorage.removeItem(KEY); } catch { /* storage unavailable */ }
  window.dispatchEvent(new Event("mdc:signed-out"));
}
export function authorized(init?: RequestInit): RequestInit | undefined {
  const token = accessToken();
  if (!token) return init;
  const headers = new Headers(init?.headers);
  headers.set("authorization", `Bearer ${token}`);
  return { ...init, headers };
}
