import { describe, expect, it } from "vitest";
import { formatShortcut, readRecording, spokenShortcut, type KeyInfo } from "./shortcut";

const keys = (code: string, mods: Partial<KeyInfo> = {}): KeyInfo => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

describe("formatShortcut", () => {
  it("writes modifiers as macOS symbols in the system order", () => {
    expect(formatShortcut("Ctrl+Alt+M")).toBe("⌃⌥M");
    expect(formatShortcut("Cmd+Shift+Ctrl+Alt+K")).toBe("⌃⌥⇧⌘K");
    expect(formatShortcut("Command+Option+1")).toBe("⌥⌘1");
  });
  it("writes named keys readably", () => {
    expect(formatShortcut("Ctrl+Shift+Space")).toBe("⌃⇧Space");
    expect(formatShortcut("Alt+Up")).toBe("⌥↑");
    expect(formatShortcut("Ctrl+F5")).toBe("⌃F5");
  });
});

describe("readRecording", () => {
  it("shows the modifiers held so far while no key is pressed", () => {
    expect(readRecording(keys("ControlLeft", { ctrlKey: true }))).toEqual({ kind: "pending", text: "⌃" });
    expect(readRecording(keys("AltLeft", { ctrlKey: true, altKey: true }))).toEqual({ kind: "pending", text: "⌃⌥" });
  });
  it("completes with a key and at least one of Control, Option or Command", () => {
    expect(readRecording(keys("KeyM", { ctrlKey: true, altKey: true }))).toEqual({ kind: "ready", shortcut: "Ctrl+Alt+M" });
    expect(readRecording(keys("Digit1", { metaKey: true }))).toEqual({ kind: "ready", shortcut: "Cmd+1" });
    expect(readRecording(keys("Space", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "ready", shortcut: "Ctrl+Shift+Space" });
    expect(readRecording(keys("F5", { altKey: true }))).toEqual({ kind: "ready", shortcut: "Alt+F5" });
    expect(readRecording(keys("ArrowUp", { altKey: true }))).toEqual({ kind: "ready", shortcut: "Alt+Up" });
  });
  it("refuses a key without a modifier, or with only Shift, and says what to add", () => {
    for (const e of [keys("KeyM"), keys("KeyM", { shiftKey: true })]) {
      const r = readRecording(e);
      expect(r.kind).toBe("invalid");
      expect(r).toMatchObject({ reason: expect.stringContaining("Control, Option, atau Command") });
    }
  });
  it("refuses keys it cannot register", () => {
    expect(readRecording(keys("Enter", { ctrlKey: true }))).toMatchObject({ kind: "invalid" });
    expect(readRecording(keys("Backquote", { ctrlKey: true }))).toMatchObject({ kind: "invalid" });
  });
});

it("spells a shortcut out for a screen reader", () => {
  expect(spokenShortcut("Ctrl+Alt+M")).toBe("Control Option M");
  expect(spokenShortcut("Cmd+Shift+Up")).toBe("Command Shift Up");
});
