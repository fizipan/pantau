/** Mirrors macOS light/dark onto `<html class="dark">`, which the colour tokens in index.css key on. Returns a cleanup. */
export function syncColorScheme(): () => void {
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  const apply = (dark: boolean) => document.documentElement.classList.toggle("dark", dark);
  const onChange = (e: { matches: boolean }) => apply(e.matches);
  apply(mq.matches);
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}
