import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Settings as SettingsData } from "@/lib/api";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { Settings } from "./Settings";

const initial: SettingsData = {
  launch_at_login: false,
  shortcut: "Ctrl+Alt+M",
  notifications: true,
  scan_folders: ["/Users/example/Documents"],
};

let server: SettingsData;
let picked: string | null;

beforeEach(() => {
  invoke.mockReset();
  server = { ...initial };
  picked = null;
  invoke.mockImplementation((cmd: string, args?: { settings: SettingsData }) => {
    if (cmd === "get_settings") return Promise.resolve({ ...server });
    if (cmd === "save_settings") {
      server = { ...args!.settings };
      return Promise.resolve({ ...server });
    }
    if (cmd === "pick_scan_folder") return Promise.resolve(picked);
    return Promise.resolve(undefined);
  });
});

const saves = () => invoke.mock.calls.filter(([c]) => c === "save_settings").map(([, a]) => (a as { settings: SettingsData }).settings);
const calls = (name: string) => invoke.mock.calls.filter(([c]) => c === name).length;
const shortcutButton = () => screen.getByRole("button", { name: /Pintasan/ });

async function open() {
  const user = userEvent.setup();
  render(<Settings />);
  await screen.findByRole("switch", { name: /Mulai saat login/ });
  return user;
}

describe("Settings", () => {
  it("settings show autostart, shortcut, notifications, and folders", async () => {
    await open();
    expect(screen.getByRole("switch", { name: /Mulai saat login/ })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: /Notifikasi/ })).toHaveAttribute("aria-checked", "true");
    expect(shortcutButton()).toHaveTextContent("⌃⌥M");
    expect(screen.getByText("~/Documents")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Tambah folder…" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox"), "folders are picked, never typed").toBeNull();
  });

  it("shows a hint instead of a list when no folder is set", async () => {
    server.scan_folders = [];
    await open();
    expect(screen.getByText(/Belum ada folder/)).toBeInTheDocument();
  });

  it("toggling a switch saves the whole form", async () => {
    const user = await open();
    await user.click(screen.getByRole("switch", { name: /Mulai saat login/ }));
    await waitFor(() => expect(saves()).toEqual([{ ...initial, launch_at_login: true }]));
    expect(screen.getByRole("switch", { name: /Mulai saat login/ })).toHaveAttribute("aria-checked", "true");
  });

  it("the shortcut button's name starts with the text it shows, and recording is announced", async () => {
    const user = await open();
    expect(shortcutButton().getAttribute("aria-label")?.startsWith("⌃⌥M")).toBe(true);
    expect(screen.queryByRole("status")).toBeEmptyDOMElement();
    await user.click(shortcutButton());
    expect(shortcutButton().getAttribute("aria-label")?.startsWith("Tekan pintasan…")).toBe(true);
    expect(screen.getByRole("status")).toHaveTextContent(/Merekam/);
    await user.keyboard("{Escape}");
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });

  it("recording a shortcut saves it", async () => {
    const user = await open();
    await user.click(shortcutButton());
    expect(shortcutButton()).toHaveTextContent("Tekan pintasan");
    await user.keyboard("{Control>}{Alt>}p{/Alt}{/Control}");
    await waitFor(() => expect(saves()).toEqual([{ ...initial, shortcut: "Ctrl+Alt+P" }]));
    await waitFor(() => expect(shortcutButton()).toHaveTextContent("⌃⌥P"));
    expect(shortcutButton()).not.toHaveTextContent("Tekan pintasan");
  });

  it("shows what is pressed while recording", async () => {
    const user = await open();
    await user.click(shortcutButton());
    await user.keyboard("{Control>}{Alt>}");
    expect(shortcutButton()).toHaveTextContent("⌃⌥");
    expect(shortcutButton()).not.toHaveTextContent("Tekan pintasan");
    await user.keyboard("{/Alt}{/Control}");
    expect(calls("save_settings")).toBe(0);
  });

  it("Escape cancels recording, keeps the old shortcut, and does not close the window", async () => {
    const user = await open();
    await user.click(shortcutButton());
    await user.keyboard("{Escape}");
    expect(shortcutButton()).toHaveTextContent("⌃⌥M");
    expect(calls("save_settings")).toBe(0);
    expect(calls("close_settings")).toBe(0);
    await user.keyboard("{Escape}");
    expect(calls("close_settings")).toBe(1);
  });

  it("a key without a modifier is not saved and says what to add", async () => {
    const user = await open();
    await user.click(shortcutButton());
    await user.keyboard("m");
    expect(await screen.findByRole("alert")).toHaveTextContent("Control, Option, atau Command");
    expect(calls("save_settings")).toBe(0);
    expect(shortcutButton()).toHaveTextContent("Tekan pintasan");
  });

  it("clicking away stops recording", async () => {
    const user = await open();
    await user.click(shortcutButton());
    await user.click(screen.getByRole("switch", { name: /Notifikasi/ }));
    expect(shortcutButton()).toHaveTextContent("⌃⌥M");
  });

  it("a shortcut that cannot be registered shows why and keeps the old one", async () => {
    const user = await open();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "save_settings") return Promise.reject("Pintasan tidak bisa didaftarkan: macOS menolak kombinasi ini. Coba kombinasi lain.");
      if (cmd === "get_settings") return Promise.resolve({ ...server });
      return Promise.resolve(undefined);
    });
    await user.click(shortcutButton());
    await user.keyboard("{Control>}{Alt>}p{/Alt}{/Control}");
    expect(await screen.findByRole("alert")).toHaveTextContent("macOS menolak");
    expect(shortcutButton()).toHaveTextContent("⌃⌥M");
  });

  it("shows why the saved shortcut was not registered at startup", async () => {
    server.shortcut_error = "Pintasan tidak bisa didaftarkan: macOS menolak kombinasi ini.";
    await open();
    expect(screen.getByRole("alert")).toHaveTextContent("macOS menolak");
  });

  it("sends the next save only after the previous one answered, and the newest value wins", async () => {
    const user = await open();
    let release: (v: SettingsData) => void = () => {};
    invoke.mockImplementation((cmd: string, args?: { settings: SettingsData }) => {
      if (cmd === "save_settings" && calls("save_settings") === 1) return new Promise<SettingsData>((r) => (release = r));
      if (cmd === "save_settings") return Promise.resolve({ ...args!.settings });
      return Promise.resolve({ ...server });
    });
    const notifications = screen.getByRole("switch", { name: /Notifikasi/ });
    await user.click(notifications); // off
    await user.click(notifications); // on again, while the first save is still running
    expect(notifications).toHaveAttribute("aria-checked", "true");
    expect(saves()).toHaveLength(1);
    await act(async () => release({ ...initial, notifications: false }));
    await waitFor(() => expect(saves()).toHaveLength(2));
    expect(saves()[1].notifications).toBe(true);
    await waitFor(() => expect(notifications).toHaveAttribute("aria-checked", "true"));
  });

  it("an edit made while a save fails is kept, and the failed one is undone", async () => {
    const user = await open();
    let fail: (e: string) => void = () => {};
    invoke.mockImplementation((cmd: string, args?: { settings: SettingsData }) => {
      if (cmd === "save_settings" && calls("save_settings") === 1) return new Promise((_, rej) => (fail = rej));
      if (cmd === "save_settings") return Promise.resolve({ ...args!.settings });
      return Promise.resolve({ ...initial });
    });
    await user.click(screen.getByRole("switch", { name: /Mulai saat login/ })); // will fail
    await user.click(screen.getByRole("switch", { name: /Notifikasi/ })); // made meanwhile
    await act(async () => fail("Mulai saat login tidak bisa diubah: ditolak"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Mulai saat login tidak bisa diubah");
    await waitFor(() => expect(saves()).toHaveLength(2));
    expect(saves()[1]).toEqual({ ...initial, launch_at_login: false, notifications: false });
    expect(screen.getByRole("switch", { name: /Mulai saat login/ })).toHaveAttribute("aria-checked", "false");
  });

  it("adds a folder from the picker, and a cancelled picker changes nothing", async () => {
    const user = await open();
    await user.click(screen.getByRole("button", { name: "Tambah folder…" }));
    await waitFor(() => expect(calls("pick_scan_folder")).toBe(1));
    expect(calls("save_settings")).toBe(0);
    picked = "/Users/example/Work";
    await user.click(screen.getByRole("button", { name: "Tambah folder…" }));
    await waitFor(() => expect(saves()).toEqual([{ ...initial, scan_folders: ["/Users/example/Documents", "/Users/example/Work"] }]));
    expect(await screen.findByText("~/Work")).toBeInTheDocument();
  });

  it("removes a folder", async () => {
    const user = await open();
    await user.click(screen.getByRole("button", { name: "Hapus ~/Documents" }));
    await waitFor(() => expect(saves()).toEqual([{ ...initial, scan_folders: [] }]));
    expect(await screen.findByText(/Belum ada folder/)).toBeInTheDocument();
  });

  it("shows why a folder was refused and puts the list back", async () => {
    const user = await open();
    picked = "/Users/example";
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "pick_scan_folder") return Promise.resolve(picked);
      if (cmd === "save_settings") return Promise.reject("/Users/example: Folder Home tidak boleh dipindai seluruhnya, pilih folder project di dalamnya");
      return Promise.resolve({ ...server });
    });
    await user.click(screen.getByRole("button", { name: "Tambah folder…" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Folder Home tidak boleh dipindai");
    await waitFor(() => expect(screen.queryByText("~")).toBeNull());
    expect(within(screen.getByRole("list")).getAllByRole("listitem")).toHaveLength(1);
  });

  it("Escape and Command+W close the window when nothing is being recorded", async () => {
    const user = await open();
    await user.keyboard("{Escape}");
    await user.keyboard("{Meta>}w{/Meta}");
    expect(calls("close_settings")).toBe(2);
  });
  it("closing waits for a save that is still running, so the last edit is not lost", async () => {
    const user = await open();
    let release: (v: SettingsData) => void = () => {};
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "save_settings") return new Promise<SettingsData>((r) => (release = r));
      return Promise.resolve({ ...server });
    });
    await user.click(screen.getByRole("button", { name: "Hapus ~/Documents" }));
    await user.keyboard("{Escape}");
    await act(async () => {});
    expect(calls("close_settings")).toBe(0);
    await act(async () => release({ ...initial, scan_folders: [] }));
    await waitFor(() => expect(calls("close_settings")).toBe(1));
  });

  it("Escape does not close the window when the last save failed, and the reason stays on screen", async () => {
    const user = await open();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "save_settings") return Promise.reject("Pintasan tidak bisa didaftarkan: macOS menolak kombinasi ini.");
      return Promise.resolve({ ...server });
    });
    await user.click(screen.getByRole("switch", { name: /Notifikasi/ }));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("macOS menolak"));
    expect(calls("close_settings")).toBe(0);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(calls("close_settings")).toBe(1));
  });

  it("an old save failure does not hold the close once the form matches what is saved again", async () => {
    const user = await open();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "save_settings") return Promise.reject("gagal");
      return Promise.resolve({ ...server });
    });
    await user.click(screen.getByRole("switch", { name: /Notifikasi/ }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    const toggle = screen.getByRole("switch", { name: /Notifikasi/ });
    // Two clicks before any save has run: the form is back to what is saved.
    act(() => void fireEvent.click(toggle));
    act(() => void fireEvent.click(toggle));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(calls("close_settings")).toBe(1));
  });

  it("a double click on Tambah folder opens one picker", async () => {
    const user = await open();
    let answer: (v: string | null) => void = () => {};
    invoke.mockImplementation((cmd: string) => (cmd === "pick_scan_folder" ? new Promise((r) => (answer = r)) : Promise.resolve({ ...server })));
    const add = screen.getByRole("button", { name: "Tambah folder…" });
    await user.dblClick(add);
    expect(calls("pick_scan_folder")).toBe(1);
    await act(async () => answer(null));
    await user.click(add);
    expect(calls("pick_scan_folder")).toBe(2);
  });

  it("Tab leaves recording instead of showing a hint", async () => {
    const user = await open();
    await user.click(shortcutButton());
    await user.keyboard("{Tab}");
    expect(shortcutButton()).toHaveTextContent("⌃⌥M");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(calls("save_settings")).toBe(0);
  });

  it("a startup shortcut error does not stop other settings from saving, and stays shown", async () => {
    const why = "Pintasan tidak bisa didaftarkan: macOS menolak kombinasi ini. Coba kombinasi lain.";
    server.shortcut_error = why;
    const user = await open();
    invoke.mockImplementation((cmd: string, args?: { settings: SettingsData }) =>
      Promise.resolve(cmd === "save_settings" ? { ...args!.settings, shortcut_error: why } : { ...server }),
    );
    await user.click(screen.getByRole("switch", { name: /Notifikasi/ }));
    await waitFor(() => expect(saves()).toEqual([{ ...initial, notifications: false }]));
    expect(screen.getByRole("alert")).toHaveTextContent("macOS menolak");
  });
});
