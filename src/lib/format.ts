const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** "1,1 GB", "471 MB": decimal comma, one digit below 100, whole numbers above. */
export function formatBytes(bytes: number): string {
  let value = bytes;
  let unit = 0;
  // Compare the rounded value, so 1023.7 KB becomes "1,0 MB" and never "1024 KB".
  while (Math.round(value) >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const oneDigit = Math.round(value * 10) / 10;
  const text = unit > 0 && oneDigit < 100 ? oneDigit.toFixed(1).replace(".", ",") : String(Math.round(value));
  return `${text} ${UNITS[unit]}`;
}

/** "12 menit lalu"; anything under a minute, or in the future, is "baru saja". */
export function formatAge(atMs: number, nowMs: number): string {
  const minutes = Math.floor((nowMs - atMs) / 60_000);
  if (minutes < 1) return "baru saja";
  if (minutes < 60) return `${minutes} menit lalu`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} jam lalu`;
  return `${Math.floor(hours / 24)} hari lalu`;
}
