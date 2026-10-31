import { act, createEvent, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Operation, StorageData } from "@/lib/api";
import { GB, item, MEASURED, recent, storage } from "@/test/storage";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { StorageTab } from "./StorageTab";

let current: StorageData = storage();
const calls = (cmd: string) => invoke.mock.calls.filter(([c]) => c === cmd);
const op = (op_id: string, state: Operation["state"]): Operation => ({ op_id, state, survivors: [] });

async function show(data: StorageData, operations: Operation[] = []) {
  current = data;
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
  const view = render(<StorageTab operations={operations} />);
  // The first getStorage answer lands after a microtask.
  await act(async () => {});
  return { user, view };
}
const tick = (ms: number) => act(async () => void vi.advanceTimersByTime(ms));

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.setSystemTime(MEASURED + 30_000);
  current = storage();
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => (cmd === "get_storage" ? Promise.resolve(current) : Promise.resolve(null)));
});
afterEach(() => vi.useRealTimers());

describe("StorageTab", () => {
  it("groups items into Aman dibersihkan and Perlu dicek dulu", async () => {
    await show(storage([item("pnpm", { title: "Cache pnpm" }), item("cursor-db", { title: "Database Cursor", category: "check", cleaner: "none" })]));
    const safe = screen.getByRole("region", { name: "Aman dibersihkan" });
    const check = screen.getByRole("region", { name: "Perlu dicek dulu" });
    expect(within(safe).getByText("Cache pnpm")).toBeInTheDocument();
    expect(within(check).getByText("Database Cursor")).toBeInTheDocument();
    expect(within(safe).queryByText("Database Cursor")).not.toBeInTheDocument();
  });

  it("section titles are level-2 headings, since the panel has no level-1 or level-3 above them", async () => {
    await show(storage([item("pnpm"), item("cursor-db", { category: "check", cleaner: "none" })]));
    for (const name of ["Aman dibersihkan", "Perlu dicek dulu", "Baru masuk atau berubah"]) {
      expect(screen.getByRole("heading", { name, level: 2 })).toBeInTheDocument();
    }
    expect(screen.queryAllByRole("heading", { level: 3 })).toHaveLength(0);
  });

  it("Ukur ulang stays usable while the measure running is an automatic one, so the owner can make it last", async () => {
    vi.setSystemTime(MEASURED + 12 * 60_000);
    const { user } = await show(storage([item("pnpm", { bytes: 4 * GB })], { measuring: true, measuringAuto: true }));
    const button = screen.getByRole("button", { name: "Ukur ulang" });
    expect(button).toBeEnabled();
    await user.click(button);
    expect(calls("refresh_storage")).toHaveLength(1);
  });

  it("Ukur ulang is disabled once the owner's own measure is running", async () => {
    vi.setSystemTime(MEASURED + 12 * 60_000);
    await show(storage([item("pnpm", { bytes: 4 * GB })], { measuring: true, measuringAuto: false }));
    expect(screen.getByRole("button", { name: "Ukur ulang" })).toBeDisabled();
  });

  it("check items only offer Lihat, never a clean button", async () => {
    const { user } = await show(storage([item("cursor-db", { title: "Database Cursor", category: "check", cleaner: "none" })]));
    expect(screen.queryByRole("button", { name: /Bersihkan/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Lihat Database Cursor/ }));
    expect(invoke).toHaveBeenCalledWith("reveal_in_finder", { id: "cursor-db" });
  });

  it("groups node_modules under one collapsed row with a count and no summed size", async () => {
    const mods = ["a", "b", "c"].map((n) =>
      item(`node-modules-/p/${n}`, { title: "node_modules", sub: `/Users/example/Documents/${n}`, path: `/Users/example/Documents/${n}/node_modules`, category: "check", cleaner: "none", bytes: 2 * GB }),
    );
    const { user } = await show(storage(mods));
    const head = screen.getByRole("button", { name: /node_modules/ });
    expect(head).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText("3 folder")).toBeInTheDocument();
    expect(screen.queryByText("6,0 GB")).not.toBeInTheDocument();
    expect(screen.queryByText("2,0 GB")).not.toBeInTheDocument();
    await user.click(head);
    expect(head).toHaveAttribute("aria-expanded", "true");
    expect(screen.getAllByText("2,0 GB")).toHaveLength(3);
    expect(screen.getAllByRole("button", { name: /^Lihat/ })).toHaveLength(3);
  });

  it("first measurement shows Mengukur… in place of the amount", async () => {
    await show(storage([item("pnpm", { title: "Cache pnpm", bytes: null, measuredAtMs: null, state: "pending" })], { measuring: true }));
    expect(screen.getByText("Mengukur…")).toBeInTheDocument();
    expect(screen.queryByText(/ B$/)).not.toBeInTheDocument();
  });

  it("stale data keeps the amount and says when it was measured", async () => {
    vi.setSystemTime(MEASURED + 12 * 60_000);
    const { user } = await show(storage([item("pnpm", { title: "Cache pnpm", bytes: 4 * GB })]));
    expect(screen.getByText("4,0 GB")).toBeInTheDocument();
    expect(screen.getByText("Diukur 12 menit lalu")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Ukur ulang" }));
    expect(calls("refresh_storage")).toHaveLength(1);
  });

  it("fresh data does not nag about its age", async () => {
    await show(storage([item("pnpm")]));
    expect(screen.queryByText(/Diukur/)).not.toBeInTheDocument();
  });

  it("upper bound items say hingga", async () => {
    await show(storage([item("pnpm", { title: "Cache pnpm", upperBound: true, bytes: 9.6 * GB })]));
    expect(screen.getByText("hingga 9,6 GB")).toBeInTheDocument();
  });

  it("cleaning asks with the path, then shows what came back", async () => {
    const path = "/Users/example/Library/pnpm/store";
    const { user, view } = await show(storage([item("pnpm", { title: "Cache pnpm", cleaner: "pnpmPrune", upperBound: true, bytes: 9.6 * GB, path })]));
    await user.click(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }));
    expect(screen.getByText(path)).toBeInTheDocument();
    expect(screen.getByText("Bisa membebaskan hingga 9,6 GB")).toBeInTheDocument();
    expect(calls("clean_item")).toHaveLength(0);
    await tick(500);
    await user.click(screen.getByRole("button", { name: /^Ya, bersihkan Cache pnpm/ }));
    expect(invoke).toHaveBeenCalledWith("clean_item", { id: "pnpm", opId: "clean:pnpm" });
    expect(screen.getByRole("button", { name: /^Membersihkan Cache pnpm/ })).toHaveAttribute("aria-disabled", "true");
    view.rerender(<StorageTab operations={[op("clean:pnpm", { Done: { freed_bytes: 7.1 * GB, skipped: 0 } })]} />);
    expect(screen.getByText("7,1 GB kembali")).toBeInTheDocument();
    expect(screen.queryByText(/dilewati/)).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("7,1 GB kembali");
  });

  it("notes the items a clean skipped", async () => {
    await show(storage([item("temp", { cleaner: "oldTemp" })]), [op("clean:temp", { Done: { freed_bytes: 1 * GB, skipped: 3 } })]);
    expect(screen.getByText("1,0 GB kembali · 3 item dilewati")).toBeInTheDocument();
  });

  it("a reopened panel shows a clean that is still running", async () => {
    await show(storage([item("temp", { title: "File sementara", cleaner: "oldTemp" })]), [op("clean:temp", "Running")]);
    expect(screen.getByRole("button", { name: /^Membersihkan File sementara/ })).toBeInTheDocument();
  });

  it("a failed clean shows its reason", async () => {
    await show(storage([item("temp")]), [op("clean:temp", { Failed: "Lokasi ini dilindungi" })]);
    expect(screen.getByText("Lokasi ini dilindungi")).toBeInTheDocument();
  });

  it("a second press right after asking is ignored", async () => {
    const { user } = await show(storage([item("pnpm", { title: "Cache pnpm" })]));
    await user.click(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }));
    await user.click(screen.getByRole("button", { name: /^Ya, bersihkan/ }));
    expect(calls("clean_item")).toHaveLength(0);
    expect(screen.getByRole("button", { name: /^Ya, bersihkan/ })).toBeInTheDocument();
  });

  it("temp items say what the rule is, and the limit of seeing open files", async () => {
    const { user } = await show(storage([item("temp", { title: "File sementara", cleaner: "oldTemp", upperBound: true })]));
    await user.click(screen.getByRole("button", { name: /^Bersihkan File sementara/ }));
    expect(screen.getByText(/Hanya file milikmu yang tidak berubah 3 hari dan tidak sedang dibuka/)).toBeInTheDocument();
    expect(screen.getByText(/tidak bisa melihat semua file yang terbuka/)).toBeInTheDocument();
  });

  it("moving away or pressing Escape reverts the question, and Escape does not reach the panel", async () => {
    const onWindow = vi.fn((e: KeyboardEvent) => e.key === "Escape" && e.defaultPrevented);
    window.addEventListener("keydown", onWindow);
    const { user } = await show(storage([item("pnpm", { title: "Cache pnpm" }), item("temp", { title: "File sementara" })]));
    await user.click(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("button", { name: /^Ya, bersihkan/ })).not.toBeInTheDocument();
    expect(onWindow).toHaveReturnedWith(true);
    await user.click(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }));
    await user.click(screen.getByRole("button", { name: /^Bersihkan File sementara/ }));
    expect(screen.getAllByRole("button", { name: /^Ya, bersihkan/ })).toHaveLength(1);
    expect(screen.getByRole("button", { name: /^Ya, bersihkan File sementara/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    onWindow.mockClear();
    await user.keyboard("{Escape}");
    expect(onWindow).toHaveReturnedWith(false);
    window.removeEventListener("keydown", onWindow);
  });

  it("a refused item shows the reason and a disabled button", async () => {
    await show(storage([item("next-2", { title: "Cache build Next.js", state: { refused: "Masih ada proses yang berjalan di project ini" } })]));
    expect(screen.getByText("Masih ada proses yang berjalan di project ini")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Bersihkan Cache build/ })).toBeDisabled();
  });

  it("denied shows the permission message, never zero", async () => {
    const { user } = await show(
      storage([item("next-1", { path: "/Users/example/Documents/app/.next", bytes: null, measuredAtMs: null, state: "denied" })]),
    );
    expect(screen.getByText("Butuh izin untuk membaca Documents")).toBeInTheDocument();
    expect(screen.queryByText(/0 B/)).not.toBeInTheDocument();
    expect(screen.queryByText("Mengukur…")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Buka Pengaturan Sistem" }));
    expect(calls("open_privacy_settings")).toHaveLength(1);
  });

  it("names a project folder it could not list, which has no rows to carry the message", async () => {
    const { user } = await show(storage([item("pnpm")], { deniedRoots: ["/Users/example/Documents", "/Users/example/Work"] }));
    expect(screen.getByText("Butuh izin untuk membaca Documents")).toBeInTheDocument();
    expect(screen.getByText("Butuh izin untuk membaca Work")).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: "Buka Pengaturan Sistem" })[0]);
    expect(calls("open_privacy_settings")).toHaveLength(1);
  });

  it("says nothing about permission when every folder could be listed", async () => {
    await show(storage([item("pnpm")]));
    expect(screen.queryByText(/Butuh izin/)).not.toBeInTheDocument();
  });

  it("an item not measured yet says so while nothing is measuring", async () => {
    await show(storage([item("pnpm", { title: "Cache pnpm", bytes: null, measuredAtMs: null, state: "pending" })], { paused: true, measuring: false }));
    expect(screen.queryByText("Mengukur…")).not.toBeInTheDocument();
    expect(screen.getByText("Belum diukur")).toBeInTheDocument();
  });

  it("critical pressure shows the paused line with a manual trigger", async () => {
    const { user } = await show(storage([item("pnpm")], { paused: true }));
    expect(screen.getByText("Dijeda karena memori penuh")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Ukur sekarang" }));
    expect(calls("refresh_storage")).toHaveLength(1);
  });

  it("a very long path stays on one line", async () => {
    const long = `/Users/example/${"very-long-folder-name/".repeat(12)}.next`;
    await show(storage([item("next-1", { title: "Cache build Next.js", sub: long, path: long })]));
    const sub = screen.getByText(long);
    expect(sub).toHaveClass("truncate");
    expect(sub).toHaveAttribute("title", long);
    expect(sub.parentElement).toHaveClass("min-w-0");
  });

  it("no items renders without crashing", async () => {
    await show(storage([]));
    expect(screen.queryByRole("region", { name: "Aman dibersihkan" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Baru masuk atau berubah" })).toBeInTheDocument();
  });

  it("keeps asking for fresh data while shown, and stops when it goes away", async () => {
    const { view } = await show(storage([item("pnpm", { bytes: null, measuredAtMs: null, state: "pending" })], { measuring: true }));
    const before = calls("get_storage").length;
    current = storage([item("pnpm", { title: "Cache pnpm", bytes: 2 * GB })]);
    await tick(3100);
    expect(calls("get_storage").length).toBeGreaterThan(before);
    expect(screen.getByText("2,0 GB")).toBeInTheDocument();
    view.unmount();
    const after = calls("get_storage").length;
    await tick(10_000);
    expect(calls("get_storage")).toHaveLength(after);
  });

  it("keeps the result of a clean whose item is gone from the list", async () => {
    const id = "next-/Users/example/Documents/app/.next";
    const done = op(`clean:${id}`, { Done: { freed_bytes: 7.1 * GB, skipped: 0 } });
    // Opened after the clean: the item was never in this list.
    await show(storage([]), [done]);
    const safe = screen.getByRole("region", { name: "Aman dibersihkan" });
    expect(within(safe).getByText("Cache build Next.js")).toBeInTheDocument();
    expect(within(safe).getByText("/Users/example/Documents/app/.next")).toBeInTheDocument();
    expect(within(safe).getByText("7,1 GB kembali")).toBeInTheDocument();
  });

  it("the result stays when the item leaves the list while it is shown", async () => {
    const id = "next-/Users/example/Documents/app/.next";
    const path = "/Users/example/Documents/app/.next";
    await show(storage([item(id, { title: "Cache build Next.js", path, sub: "/Users/example/Documents/app" })]), [op(`clean:${id}`, { Done: { freed_bytes: 7.1 * GB, skipped: 0 } })]);
    current = storage([]);
    await tick(3100);
    expect(screen.getByText("7,1 GB kembali")).toBeInTheDocument();
    expect(screen.getByText("Cache build Next.js")).toBeInTheDocument();
  });

  it("a failed clean of a vanished item also stays visible", async () => {
    await show(storage([]), [op("clean:next-/a/.next", { Failed: "Folder ini tidak dihapus" })]);
    expect(screen.getByText("Folder ini tidak dihapus")).toBeInTheDocument();
  });

  it("says so on the row when Finder cannot show a location", async () => {
    invoke.mockImplementation((cmd: string) => (cmd === "get_storage" ? Promise.resolve(current) : cmd === "reveal_in_finder" ? Promise.reject("macOS tidak bisa membuka lokasi ini") : Promise.resolve(null)));
    const { user } = await show(storage([item("cursor-db", { title: "Database Cursor", category: "check", cleaner: "none" })]));
    await user.click(screen.getByRole("button", { name: /^Lihat Database Cursor/ }));
    expect(await screen.findByText("macOS tidak bisa membuka lokasi ini")).toBeInTheDocument();
  });

  it("an older poll answer cannot overwrite a newer one", async () => {
    let first!: (d: StorageData) => void;
    let n = 0;
    invoke.mockImplementation((cmd: string) => {
      if (cmd !== "get_storage") return Promise.resolve(null);
      return ++n === 1 ? new Promise((r) => (first = r)) : Promise.resolve(current);
    });
    current = storage([item("pnpm", { title: "Cache pnpm", bytes: 2 * GB })]);
    render(<StorageTab operations={[]} />);
    await tick(3100);
    expect(screen.getByText("2,0 GB")).toBeInTheDocument();
    await act(async () => first(storage([item("pnpm", { title: "Cache pnpm", bytes: 1 * GB })])));
    expect(screen.getByText("2,0 GB")).toBeInTheDocument();
  });

  it("a second clean does not flicker back to the first result", async () => {
    const done = { Done: { freed_bytes: 7.1 * GB, skipped: 0 } };
    const { user, view } = await show(storage([item("pnpm", { title: "Cache pnpm" })]), [op("clean:pnpm", done)]);
    await user.click(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }));
    await tick(500);
    await user.click(screen.getByRole("button", { name: /^Ya, bersihkan Cache pnpm/ }));
    // The next snapshot still carries the old result, as a new object.
    view.rerender(<StorageTab operations={[op("clean:pnpm", { Done: { ...done.Done } })]} />);
    expect(screen.getByRole("button", { name: /^Membersihkan Cache pnpm/ })).toBeInTheDocument();
    expect(screen.queryByText("7,1 GB kembali")).not.toBeInTheDocument();
  });

  it("shows Mengukur… instead of the old amount while a finished clean is remeasured", async () => {
    await show(storage([item("pnpm", { title: "Cache pnpm", bytes: 9.6 * GB })], { measuring: true }), [op("clean:pnpm", { Done: { freed_bytes: 7.1 * GB, skipped: 0 } })]);
    expect(screen.getByText("7,1 GB kembali")).toBeInTheDocument();
    expect(screen.getByText("Mengukur…")).toBeInTheDocument();
    expect(screen.queryByText("9,6 GB")).not.toBeInTheDocument();
  });

  it("a held Enter cannot confirm", async () => {
    await show(storage([item("pnpm", { title: "Cache pnpm" })]));
    const ev = createEvent.keyDown(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }), { key: "Enter", repeat: true });
    fireEvent(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }), ev);
    expect(ev.defaultPrevented).toBe(true);
  });

  it("shows recent files below the items", async () => {
    await show(storage([item("pnpm")], { recent: [recent("film.mov")] }));
    expect(within(screen.getByRole("region", { name: "Baru masuk atau berubah" })).getByText("film.mov")).toBeInTheDocument();
  });
});
