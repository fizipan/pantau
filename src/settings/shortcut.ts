/** Turns the keys of a shortcut recording into the string Rust registers, and back into macOS symbols. */

export type Recording =
  | { kind: "pending"; text: string }
  | { kind: "ready"; shortcut: string }
  | { kind: "invalid"; reason: string };

export interface KeyInfo {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

const MOD_SYMBOLS: Record<string, string> = { ctrl: "⌃", control: "⌃", alt: "⌥", option: "⌥", shift: "⇧", cmd: "⌘", command: "⌘", super: "⌘" };
/** The order macOS itself writes modifiers in. */
const MOD_ORDER = ["⌃", "⌥", "⇧", "⌘"];
const KEY_SYMBOLS: Record<string, string> = { Up: "↑", Down: "↓", Left: "←", Right: "→" };

/** "Ctrl+Alt+M" as "⌃⌥M". */
export function formatShortcut(shortcut: string): string {
  const parts = shortcut.split("+");
  const key = parts.pop() ?? "";
  const mods = parts.map((m) => MOD_SYMBOLS[m.toLowerCase()] ?? m);
  mods.sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
  return mods.join("") + (KEY_SYMBOLS[key] ?? key);
}

const ARROWS: Record<string, string> = { ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right" };

/** The key name Rust accepts for a physical key, or null for keys we do not offer. */
function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  if (/^F([1-9]|1[0-2])$/.test(code)) return code;
  if (code === "Space") return "Space";
  return ARROWS[code] ?? null;
}

const MODIFIER_KEYS = /^(Control|Alt|Shift|Meta)(Left|Right)$/;

/** What a keydown means while recording: modifiers so far, a finished shortcut, or a reason it cannot be one. */
export function readRecording(e: KeyInfo): Recording {
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Cmd"].filter(Boolean) as string[];
  if (MODIFIER_KEYS.test(e.code)) return { kind: "pending", text: formatShortcut(mods.join("+") + "+") };
  const key = keyName(e.code);
  if (!key) return { kind: "invalid", reason: "Tombol ini tidak bisa dipakai. Pilih huruf, angka, F1 sampai F12, Spasi, atau panah." };
  if (!e.ctrlKey && !e.altKey && !e.metaKey) return { kind: "invalid", reason: "Tambahkan Control, Option, atau Command." };
  return { kind: "ready", shortcut: [...mods, key].join("+") };
}

const SPOKEN: Record<string, string> = { ctrl: "Control", control: "Control", alt: "Option", option: "Option", shift: "Shift", cmd: "Command", command: "Command", super: "Command" };

/** "Ctrl+Alt+M" as "Control Option M", for a screen reader that would stumble on the symbols. */
export function spokenShortcut(shortcut: string): string {
  return shortcut.split("+").map((p) => SPOKEN[p.toLowerCase()] ?? p).join(" ");
}
