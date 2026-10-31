import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import fixture from "@/test/fixtures/panel-data.json";
import type { PanelData } from "@/lib/api";

const invoke = vi.fn();
let emit: (data: PanelData) => void = () => {};
const unlisten = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (_name: string, cb: (e: { payload: PanelData }) => void) => {
    emit = (payload) => cb({ payload });
    return Promise.resolve(unlisten);
  },
}));

import { Panel } from "./Panel";

const data = fixture as PanelData;

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => Promise.resolve(cmd === "get_snapshot" ? data : undefined));
});

describe("Panel", () => {
  it("calls panel_ready once, only after the first snapshot lands", async () => {
    let resolve: (d: PanelData) => void = () => {};
    invoke.mockImplementation((cmd: string) =>
      cmd === "get_snapshot" ? new Promise<PanelData>((r) => (resolve = r)) : Promise.resolve(undefined),
    );
    render(<Panel />);
    expect(invoke.mock.calls.filter(([c]) => c === "panel_ready")).toHaveLength(0);
    await act(async () => resolve(data));
    await screen.findAllByText("Tekanan memori meningkat");
    act(() => emit(data));
    expect(invoke.mock.calls.filter(([c]) => c === "panel_ready")).toHaveLength(1);
  });

  it("shows the window anyway when get_snapshot fails", async () => {
    invoke.mockImplementation((cmd: string) =>
      cmd === "get_snapshot" ? Promise.reject(new Error("x")) : Promise.resolve(undefined),
    );
    render(<Panel />);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("panel_ready"));
  });

  it("shows the window after 300 ms even if nothing arrives", () => {
    vi.useFakeTimers();
    try {
      invoke.mockImplementation(() => new Promise(() => {}));
      render(<Panel />);
      act(() => void vi.advanceTimersByTime(299));
      expect(invoke).not.toHaveBeenCalledWith("panel_ready");
      act(() => void vi.advanceTimersByTime(2));
      expect(invoke.mock.calls.filter(([c]) => c === "panel_ready")).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows the banner from the snapshot and follows later snapshots", async () => {
    render(<Panel />);
    expect(await screen.findByText("Swap 2,0 GB")).toBeInTheDocument();
    act(() => emit({ ...data, history: data.history.map((s) => ({ ...s, pressure: "normal" as const })) }));
    await waitFor(() => expect(screen.queryByText("Tekanan memori meningkat")).not.toBeInTheDocument());
  });

  it("tabs switch with click and keep the choice in the panel", async () => {
    const user = userEvent.setup();
    render(<Panel />);
    const proses = await screen.findByRole("tab", { name: "Proses" });
    const storage = screen.getByRole("tab", { name: "Penyimpanan" });
    expect(proses).toHaveAttribute("aria-selected", "true");
    await user.click(storage);
    expect(storage).toHaveAttribute("aria-selected", "true");
    expect(proses).toHaveAttribute("aria-selected", "false");
    expect(screen.getByRole("tabpanel")).toHaveAccessibleName("Penyimpanan");
    expect(screen.getByRole("tabpanel")).toHaveAttribute("tabindex", "0");
    expect(localStorage.length).toBe(0);
  });

  it("tabs move with the arrow keys", async () => {
    const user = userEvent.setup();
    render(<Panel />);
    const proses = await screen.findByRole("tab", { name: "Proses" });
    proses.focus();
    await user.keyboard("{ArrowRight}");
    const storage = screen.getByRole("tab", { name: "Penyimpanan" });
    expect(storage).toHaveFocus();
    expect(storage).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowRight}");
    expect(proses).toHaveFocus();
  });

  it("Escape asks Rust to close the panel", async () => {
    const user = userEvent.setup();
    render(<Panel />);
    await screen.findByRole("tab", { name: "Proses" });
    await user.keyboard("{Escape}");
    expect(invoke).toHaveBeenCalledWith("close_panel");
  });

  it("Escape clears the search first, and closes the panel only on the next press", async () => {
    const user = userEvent.setup();
    render(<Panel />);
    await user.type(await screen.findByRole("textbox", { name: "Cari proses" }), "zz");
    await user.keyboard("{Escape}");
    expect(screen.getByRole("textbox", { name: "Cari proses" })).toHaveValue("");
    expect(invoke).not.toHaveBeenCalledWith("close_panel");
    await user.keyboard("{Escape}");
    expect(invoke).toHaveBeenCalledWith("close_panel");
  });

  it("footer opens Activity Monitor and settings", async () => {
    const user = userEvent.setup();
    render(<Panel />);
    await user.click(await screen.findByRole("button", { name: "Buka Activity Monitor" }));
    expect(invoke).toHaveBeenCalledWith("open_activity_monitor");
    await user.click(screen.getByRole("button", { name: "Pengaturan" }));
    expect(invoke).toHaveBeenCalledWith("open_settings");
  });

  it("renders before the first snapshot arrives", () => {
    invoke.mockImplementation(() => new Promise(() => {}));
    render(<Panel />);
    expect(screen.getByRole("tab", { name: "Proses" })).toBeInTheDocument();
    expect(screen.queryByText("Tekanan memori meningkat")).not.toBeInTheDocument();
  });
});
