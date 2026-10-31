import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RecentFile } from "@/lib/api";
import { MEASURED, recent as base } from "@/test/storage";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { RecentList } from "./RecentList";

// Local noon, so "today", "yesterday" and the date do not depend on the time zone the tests run in.
const NOW = new Date(2026, 9, 31, 12).getTime();
const recent = (name: string, o: Partial<RecentFile> = {}) => base(name, { changedAtSecs: NOW / 1000 - 3600, ...o });
const props = { recent: [] as RecentFile[], recentAtMs: MEASURED, skippedDirs: 0, denied: false, scanning: false, scanningAuto: false, now: NOW };
const calls = (cmd: string) => invoke.mock.calls.filter(([c]) => c === cmd);

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(null);
});

describe("RecentList", () => {
  it("shows date, size, and location, and never claims a download", () => {
    render(<RecentList {...props} recent={[recent("film.mov", { path: "/Users/example/Downloads/film.mov", bytes: 3.2 * 1024 ** 3 })]} />);
    expect(screen.getByText("film.mov")).toBeInTheDocument();
    expect(screen.getByText("3,2 GB")).toBeInTheDocument();
    expect(screen.getByText("Hari ini · ~/Downloads")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/diunduh/i);
  });

  it("labels the owner as a guess", () => {
    render(<RecentList {...props} recent={[recent("a.bin", { ownerGuess: "Claude Code" })]} />);
    expect(screen.getByText("Kemungkinan dari Claude Code")).toBeInTheDocument();
  });

  it("says Kemarin and the date for older files", () => {
    const day = 86_400;
    render(<RecentList {...props} recent={[recent("a.bin", { changedAtSecs: NOW / 1000 - day }), recent("b.bin", { changedAtSecs: NOW / 1000 - day * 4 })]} />);
    expect(screen.getByText(/^Kemarin · /)).toBeInTheDocument();
    expect(screen.getByText(/^\d{1,2} \w{3} · /)).toBeInTheDocument();
  });

  it("empty message", () => {
    render(<RecentList {...props} />);
    expect(screen.getByText("Tidak ada file besar baru dalam 7 hari")).toBeInTheDocument();
  });

  it("an unreadable scan root is not an empty list", async () => {
    render(<RecentList {...props} denied />);
    expect(screen.queryByText("Tidak ada file besar baru dalam 7 hari")).not.toBeInTheDocument();
    expect(screen.getByText("Butuh izin untuk membaca folder yang dipindai")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Buka Pengaturan Sistem" }));
    expect(calls("open_privacy_settings")).toHaveLength(1);
  });

  it("notes skipped folders so an empty list is not read as nothing there", () => {
    render(<RecentList {...props} skippedDirs={2} />);
    expect(screen.getByText("2 folder tidak bisa dibaca, jadi daftar ini bisa kurang lengkap")).toBeInTheDocument();
  });

  it("a file that has never been scanned says so instead of claiming nothing", () => {
    render(<RecentList {...props} recentAtMs={null} />);
    expect(screen.queryByText("Tidak ada file besar baru dalam 7 hari")).not.toBeInTheDocument();
    expect(screen.getByText("Belum dipindai")).toBeInTheDocument();
  });

  it("Lihat reveals by index, and a scan can be asked for", async () => {
    const user = userEvent.setup();
    render(<RecentList {...props} recent={[recent("a.bin"), recent("b.bin")]} />);
    await user.click(screen.getByRole("button", { name: "Lihat b.bin" }));
    expect(invoke).toHaveBeenCalledWith("reveal_in_finder", { id: `recent:${MEASURED}:1` });
    await user.click(screen.getByRole("button", { name: "Pindai ulang" }));
    expect(calls("scan_recent_now")).toHaveLength(1);
  });

  it("the button works while the scan running is an automatic one, and not once it is the owner's", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<RecentList {...props} scanning scanningAuto />);
    const button = screen.getByRole("button", { name: "Selesaikan pindai" });
    expect(button).toBeEnabled();
    await user.click(button);
    expect(calls("scan_recent_now")).toHaveLength(1);
    rerender(<RecentList {...props} scanning scanningAuto={false} />);
    expect(screen.getByRole("button", { name: "Memindai…" })).toBeDisabled();
  });

  it("says so on the row when Finder cannot show the file", async () => {
    invoke.mockRejectedValueOnce("macOS tidak bisa membuka lokasi ini");
    render(<RecentList {...props} recent={[recent("gone.bin")]} />);
    await userEvent.click(screen.getByRole("button", { name: "Lihat gone.bin" }));
    expect(await screen.findByText("macOS tidak bisa membuka lokasi ini")).toBeInTheDocument();
  });

  it("a long path stays on one line", () => {
    const long = `/Users/example/${"deep-folder/".repeat(15)}big.bin`;
    render(<RecentList {...props} recent={[recent("big.bin", { path: long })]} />);
    const where = screen.getByText(/· ~\/.*deep-folder/);
    expect(where).toHaveClass("truncate");
    expect(where.parentElement).toHaveClass("min-w-0");
  });
});
