export type Theme = "light" | "dark";

const STORAGE_KEY = "mdc-theme";

function storedTheme(): Theme | null {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    return stored === "light" || stored === "dark" ? stored : null;
  } catch {
    return null;
  }
}

export function preferredTheme(): Theme {
  const stored = storedTheme();
  if (stored) return stored;
  return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function currentTheme(): Theme {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

export function applyTheme(theme: Theme, persist = true) {
  document.documentElement.dataset.theme = theme;
  document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')
    ?.setAttribute("content", theme === "light" ? "#edf2f8" : "#090d14");
  if (!persist) return;
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // The active page can still use the selected theme without persistence.
  }
}

export function observeTheme(onChange: (theme: Theme) => void): () => void {
  const media = matchMedia("(prefers-color-scheme: light)");
  const onMediaChange = () => {
    if (!storedTheme()) onChange(media.matches ? "light" : "dark");
  };
  const onStorage = (event: StorageEvent) => {
    if (event.key === STORAGE_KEY) onChange(preferredTheme());
  };
  media.addEventListener("change", onMediaChange);
  window.addEventListener("storage", onStorage);
  return () => {
    media.removeEventListener("change", onMediaChange);
    window.removeEventListener("storage", onStorage);
  };
}
