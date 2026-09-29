import { useEffect, useState } from "react";
import { applyTheme, currentTheme, observeTheme } from "../lib/theme";

export function useTheme() {
  const [theme, setTheme] = useState(currentTheme);
  useEffect(
    () =>
      observeTheme((next) => {
        setTheme(next);
        applyTheme(next, false);
      }),
    [],
  );
  const toggleTheme = () =>
    setTheme((previous) => {
      const next = previous === "dark" ? "light" : "dark";
      applyTheme(next);
      return next;
    });
  return { theme, toggleTheme };
}
