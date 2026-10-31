import { beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import panelFixture from "@/test/fixtures/panel-data.json";
import storageFixture from "@/test/fixtures/storage-data.json";
import * as api from "./api";
import type { PanelData, StorageData } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

test("the fixtures written by Rust have exactly the fields the types name", () => {
  const panel: PanelData = panelFixture as PanelData;
  const storage: StorageData = storageFixture as StorageData;
  const keys = (o: object) => Object.keys(o).sort();
  expect(keys(panel)).toEqual(["cpu_pending", "groups", "history", "leftovers", "operations"]);
  expect(keys(panel.history[0])).toEqual(
    ["at_ms", "core_count", "cpu_pct", "disk_free", "disk_total", "disk_used", "pressure", "ram_total", "ram_used", "swap_total", "swap_used"],
  );
  expect(keys(panel.groups[0])).toEqual(["bundle", "cpu_pct", "footprint", "id", "kind", "name", "ports", "processes", "project", "session_count"]);
  expect(keys(panel.groups[0].processes[0])).toEqual(["cpu_pct", "footprint", "killable", "label", "pid", "ports", "start_time"]);
  expect(keys(panel.leftovers[0])).toEqual(["age_secs", "command", "footprint", "kind", "pid", "preselected", "project", "start_time"]);
  expect(keys(storage)).toEqual(
    ["deniedRoots", "items", "measuring", "measuringAuto", "paused", "recent", "recentAtMs", "recentDenied", "recentSkippedDirs", "scanning", "scanningAuto"],
  );
  expect(keys(storage.items[0])).toEqual(
    ["bytes", "category", "cleaner", "id", "measuredAtMs", "path", "state", "sub", "title", "upperBound"],
  );
  expect(keys(storage.recent[0])).toEqual(["bytes", "changedAtSecs", "ownerGuess", "path"]);
  expect(storage.items.map((i) => i.state)).toContainEqual({ refused: "Masih ada proses yang berjalan di project ini" });
});

test("operations in the fixture have the OpState shapes the UI relies on", () => {
  const ops = (panelFixture as PanelData).operations;
  const byId = Object.fromEntries(ops.map((o) => [o.op_id, o]));
  expect(Object.keys(byId["op-running"]).sort()).toEqual(["op_id", "state", "survivors"]);
  expect(byId["op-running"].state).toBe("Running");
  expect(byId["op-cleaned"].state).toEqual({ Done: { freed_bytes: 2_147_483_648, skipped: 1 } });
  expect(byId["op-killed"].state).toEqual({ Done: { freed_bytes: null, skipped: 0 } });
  expect(byId["op-stuck"].state).toBe("StillAlive");
  expect(byId["op-stuck"].survivors).toEqual([{ pid: 4401, start_time: 1_793_000_401 }]);
  expect(byId["op-failed"].state).toEqual({ Failed: expect.stringContaining("gagal") });
});

test("the kill wrappers send the targets, the op id and the confirmation", async () => {
  const t = { pid: 7, start_time: 100 };
  await api.startKill([t], "op1", true);
  expect(invokeMock).toHaveBeenLastCalledWith("start_kill", { targets: [t], opId: "op1", confirmed: true });
  await api.forceKill([t], "op1");
  expect(invokeMock).toHaveBeenLastCalledWith("force_kill", { targets: [t], opId: "op1" });
  await api.planKill([t]);
  expect(invokeMock).toHaveBeenLastCalledWith("plan_kill", { targets: [t] });
});

test("clean and reveal send an id, never a path", async () => {
  await api.cleanItem("next-abc", "op2");
  expect(invokeMock).toHaveBeenLastCalledWith("clean_item", { id: "next-abc", opId: "op2" });
  await api.revealInFinder("recent:1793000000000:2");
  expect(invokeMock).toHaveBeenLastCalledWith("reveal_in_finder", { id: "recent:1793000000000:2" });
});

test("the plain wrappers call the command of the same name", async () => {
  await api.getSnapshot();
  await api.getStorage();
  await api.refreshStorage();
  await api.scanRecentNow();
  await api.openActivityMonitor();
  await api.openPrivacySettings();
  await api.openSettings();
  await api.panelReady();
  await api.closePanel();
  await api.appIcon("/Applications/Made Up.app");
  await api.getSettings();
  await api.saveSettings({ launch_at_login: false, shortcut: "Ctrl+Alt+M", notifications: true, scan_folders: ["/Users/a/Documents"] });
  await api.pickScanFolder();
  await api.closeSettings();
  expect(invokeMock.mock.calls.map((c) => c[0])).toEqual([
    "get_snapshot", "get_storage", "refresh_storage", "scan_recent_now", "open_activity_monitor",
    "open_privacy_settings", "open_settings", "panel_ready", "close_panel", "app_icon", "get_settings", "save_settings",
    "pick_scan_folder", "close_settings",
  ]);
  expect(invokeMock).toHaveBeenCalledWith("save_settings", {
    settings: { launch_at_login: false, shortcut: "Ctrl+Alt+M", notifications: true, scan_folders: ["/Users/a/Documents"] },
  });
});

test("onSnapshot hands the payload to the callback and returns the unlisten function", async () => {
  const unlisten = vi.fn();
  vi.mocked(listen).mockImplementation(async (_event, handler) => {
    (handler as (e: { payload: unknown }) => void)({ payload: { groups: [], leftovers: [], history: [], operations: [], cpu_pending: false } });
    return unlisten;
  });
  const seen: unknown[] = [];
  const off = await api.onSnapshot((s) => seen.push(s));
  expect(listen).toHaveBeenCalledWith("snapshot", expect.any(Function));
  expect(seen).toEqual([{ groups: [], leftovers: [], history: [], operations: [], cpu_pending: false }]);
  expect(off).toBe(unlisten);
});
