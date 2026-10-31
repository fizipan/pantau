import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { KillPlan, Leftover, Operation, PanelData, ProcessGroup, ProcessRow } from "@/lib/api";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { ProcessTab } from "./ProcessTab";

const GB = 1024 ** 3;
const proc = (pid: number, label: string, o: Partial<ProcessRow> = {}): ProcessRow => ({
  pid,
  start_time: pid * 10,
  label,
  footprint: 100 * 1024 ** 2,
  cpu_pct: 1,
  ports: [],
  killable: true,
  ...o,
});
const group = (id: string, name: string, o: Partial<ProcessGroup> = {}): ProcessGroup => ({
  id,
  kind: "other",
  name,
  project: null,
  bundle: null,
  footprint: 1 * GB,
  cpu_pct: 1,
  ports: [],
  processes: [proc(Number(id.replace(/\D/g, "")) || 1, name)],
  session_count: 0,
  ...o,
});
const data = (groups: ProcessGroup[], o: Partial<PanelData> = {}): PanelData => ({
  history: [],
  groups,
  leftovers: [],
  operations: [],
  cpu_pending: false,
  ...o,
});
const plan = (p: Partial<KillPlan> = {}): KillPlan => ({ needs_confirm: false, warnings: [], ...p });
const op = (op_id: string, state: Operation["state"], survivors: Operation["survivors"] = []): Operation => ({
  op_id,
  state,
  survivors,
});

let planResult: KillPlan | Error = plan();
const calls = (cmd: string) => invoke.mock.calls.filter(([c]) => c === cmd);
const rows = () => screen.getAllByRole("button", { name: /\d+ proses|sesi|:\d+/ }).map((b) => b.textContent);

beforeEach(() => {
  planResult = plan();
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => {
    if (cmd === "plan_kill") return planResult instanceof Error ? Promise.reject(planResult) : Promise.resolve(planResult);
    return Promise.resolve(null);
  });
});
afterEach(() => vi.useRealTimers());

const many = () => [
  group("app:Chrome", "Chrome", { footprint: 3 * GB, cpu_pct: 9, processes: [proc(11, "Chrome Helper"), proc(12, "Chrome GPU")] }),
  group("test:Vitest:TOOLS-LAB", "Vitest", {
    kind: "test",
    project: "TOOLS-LAB",
    footprint: 2 * GB,
    cpu_pct: 70,
    processes: [proc(21, "worker 1"), proc(22, "worker 2")],
  }),
  group("dev:next:cms", "next dev", { kind: "dev", project: "toko-contoh", footprint: 1 * GB, cpu_pct: 20, ports: [3000], processes: [proc(31, "next dev", { ports: [3000] })] }),
];

describe("ProcessTab list and search", () => {
  it("shows the top 8 groups sorted by RAM, then CPU when switched", async () => {
    const groups = Array.from({ length: 10 }, (_, i) =>
      group(`g:${i}`, `App ${i}`, { footprint: (i + 1) * GB, cpu_pct: i === 0 ? 99 : 1 }),
    );
    render(<ProcessTab data={data(groups)} />);
    const heads = () => screen.getAllByRole("button", { name: /^App \d/ });
    expect(heads()).toHaveLength(8);
    expect(heads()[0]).toHaveTextContent("App 9");
    expect(screen.queryByText("App 0")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "CPU" }));
    expect(heads()[0]).toHaveTextContent("App 0");
    expect(screen.getByRole("button", { name: "CPU" })).toHaveAttribute("aria-pressed", "true");
  });

  it("shows RAM with a decimal comma and CPU as the secondary amount", () => {
    render(<ProcessTab data={data([group("g:1", "Chrome", { footprint: 3.1 * GB, cpu_pct: 9 })])} />);
    expect(screen.getByText("3,1 GB")).toBeInTheDocument();
    expect(screen.getByText("9%")).toBeInTheDocument();
  });

  it("search matches name, project, and port", async () => {
    const user = userEvent.setup();
    render(<ProcessTab data={data(many())} />);
    const box = screen.getByRole("textbox", { name: "Cari proses" });
    await user.type(box, ":3000");
    expect(screen.getByText("next dev")).toBeInTheDocument();
    expect(screen.queryByText("Chrome")).not.toBeInTheDocument();
    expect(screen.queryByText("Vitest")).not.toBeInTheDocument();
    await user.clear(box);
    await user.type(box, "tools-lab");
    expect(screen.getByText("Vitest")).toBeInTheDocument();
    expect(screen.queryByText("next dev")).not.toBeInTheDocument();
    await user.clear(box);
    await user.type(box, "chrome");
    expect(screen.getByText("Chrome")).toBeInTheDocument();
  });

  it("search reaches unreadable processes, shown without amount or kill button", async () => {
    const hidden = group("app:Ghost", "Ghost", {
      footprint: 0,
      processes: [proc(51, "ghostd", { footprint: null, killable: false })],
    });
    render(<ProcessTab data={data([...many(), hidden])} />);
    expect(screen.queryByText("Ghost")).not.toBeInTheDocument();
    await userEvent.type(screen.getByRole("textbox", { name: "Cari proses" }), "ghostd");
    const row = screen.getByText("ghostd").closest("li")!;
    expect(row).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /^Kill/ })).not.toBeInTheDocument();
    expect(within(row).queryByText(/GB|MB|KB|%/)).not.toBeInTheDocument();
  });

  it("no match shows the message", async () => {
    render(<ProcessTab data={data(many())} />);
    await userEvent.type(screen.getByRole("textbox", { name: "Cari proses" }), "zzzz");
    expect(screen.getByText("Tidak ada proses yang cocok")).toBeInTheDocument();
  });

  it("no groups renders without crashing", () => {
    render(<ProcessTab data={data([])} />);
    expect(screen.getByRole("textbox", { name: "Cari proses" })).toBeInTheDocument();
  });

  it("says it is loading until the first process list arrives, also while searching", async () => {
    const { rerender } = render(<ProcessTab data={data([])} />);
    expect(screen.getByText("Memuat proses…")).toBeInTheDocument();
    await userEvent.type(screen.getByRole("textbox", { name: "Cari proses" }), "zzzz");
    expect(screen.getByText("Memuat proses…")).toBeInTheDocument();
    expect(screen.queryByText("Tidak ada proses yang cocok")).not.toBeInTheDocument();
    rerender(<ProcessTab data={data(many())} />);
    expect(screen.queryByText("Memuat proses…")).not.toBeInTheDocument();
    expect(screen.getByText("Tidak ada proses yang cocok")).toBeInTheDocument();
  });

  it("a search that matches a group reaches the root processes inside it", async () => {
    const user = userEvent.setup();
    const mixed = group("other:launchd", "launchd", {
      processes: [proc(61, "launchd helper"), proc(62, "launchd", { footprint: null, killable: false })],
    });
    render(<ProcessTab data={data([mixed])} />);
    await user.click(screen.getByRole("button", { name: /^launchd/ }));
    expect(screen.getByText("launchd helper")).toBeInTheDocument();
    expect(screen.queryAllByText("launchd")).toHaveLength(1);
    await user.type(screen.getByRole("textbox", { name: "Cari proses" }), "launchd");
    const rows = screen.getAllByText("launchd");
    expect(rows).toHaveLength(2);
    const root = rows.map((r) => r.closest("li")!).find((li) => li.closest("ul ul"))!;
    expect(within(root).queryByRole("button", { name: /^Kill/ })).not.toBeInTheDocument();
    expect(within(root).queryByText(/GB|MB|KB|%/)).not.toBeInTheDocument();
  });

  it("a 200-character project name stays on one line and keeps the button", () => {
    const long = "p".repeat(200);
    render(<ProcessTab data={data([group("g:1", "n".repeat(200), { project: long })])} />);
    const head = screen.getByRole("button", { name: /^n{200}/ });
    expect(head).toHaveClass("min-w-0");
    expect(within(head).getByText(new RegExp(long))).toHaveClass("truncate");
    expect(within(head).getByText("n".repeat(200))).toHaveClass("truncate");
    expect(screen.getByRole("button", { name: /^Kill/ })).toBeInTheDocument();
  });

  it("expands a group to its readable processes", async () => {
    const g = group("g:1", "Vitest", {
      processes: [proc(1, "worker 1"), proc(2, "worker 2"), proc(3, "hidden", { footprint: null, killable: false })],
    });
    render(<ProcessTab data={data([g])} />);
    await userEvent.click(screen.getByRole("button", { name: /^Vitest/ }));
    expect(screen.getByText("worker 1")).toBeInTheDocument();
    expect(screen.getByText("worker 2")).toBeInTheDocument();
    expect(screen.queryByText("hidden")).not.toBeInTheDocument();
  });

  it("uses a real app icon once per bundle, null included, not on every snapshot", async () => {
    invoke.mockImplementation((cmd: string, args?: { bundle?: string }) =>
      cmd === "app_icon" ? Promise.resolve(args?.bundle === "/A.app" ? "data:image/png;base64,AAAA" : null) : Promise.resolve(null),
    );
    const groups = [group("a", "Alpha", { kind: "app", bundle: "/A.app" }), group("b", "Beta", { kind: "app", bundle: "/B.app" })];
    const { rerender, container } = render(<ProcessTab data={data(groups)} />);
    await act(async () => {});
    expect(container.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,AAAA");
    rerender(<ProcessTab data={data(groups.map((g) => ({ ...g })))} />);
    await act(async () => {});
    expect(calls("app_icon")).toHaveLength(2);
  });
});

describe("ProcessTab kill", () => {
  it("a plain process dies on one click", async () => {
    render(<ProcessTab data={data([group("g:7", "Solo", { processes: [proc(7, "Solo")] })])} />);
    await userEvent.click(screen.getByRole("button", { name: /^Kill/ }));
    expect(calls("plan_kill")).toHaveLength(1);
    expect(calls("start_kill")).toEqual([["start_kill", { targets: [{ pid: 7, start_time: 70 }], opId: "kill:g:7", confirmed: false }]]);
  });

  it("targets only killable processes, and a group with none has no Kill button", async () => {
    const g = group("g:1", "Mixed", { processes: [proc(1, "a"), proc(2, "b", { killable: false })] });
    const none = group("g:9", "Locked", { processes: [proc(9, "c", { killable: false })] });
    render(<ProcessTab data={data([g, none])} />);
    expect(screen.queryByRole("button", { name: "Kill Locked" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Kill Mixed" }));
    expect(invoke).toHaveBeenCalledWith("plan_kill", { targets: [{ pid: 1, start_time: 10 }] });
  });

  it("a process inside a group uses its pid as the op id", async () => {
    render(<ProcessTab data={data(many())} />);
    await userEvent.click(screen.getByRole("button", { name: /^Vitest/ }));
    await userEvent.click(screen.getByRole("button", { name: "Kill worker 2" }));
    expect(calls("start_kill")[0][1]).toMatchObject({ opId: "kill:22", confirmed: false });
  });

  it("a group needs Yakin? and reverts on its own", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    planResult = plan({ needs_confirm: true });
    render(<ProcessTab data={data(many())} />);
    await user.click(screen.getByRole("button", { name: "Kill Vitest" }));
    const sure = await screen.findByRole("button", { name: /^Yakin\?/ });
    expect(sure).toHaveTextContent("Yakin?");
    expect(calls("start_kill")).toHaveLength(0);
    act(() => void sure.blur());
    expect(screen.queryByRole("button", { name: /^Yakin\?/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Kill Vitest" })).toHaveTextContent("Kill");
  });

  it("Yakin? waits while focus is on the row, and otherwise lasts about 8 seconds", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    planResult = plan({ needs_confirm: true });
    render(<ProcessTab data={data(many())} />);
    // Pointer only: the button never takes focus.
    fireEvent.click(screen.getByRole("button", { name: "Kill Vitest" }));
    await screen.findByRole("button", { name: /^Yakin\?/ });
    act(() => void vi.advanceTimersByTime(7500));
    expect(screen.getByRole("button", { name: /^Yakin\?/ })).toBeInTheDocument();
    act(() => void vi.advanceTimersByTime(1000));
    expect(screen.queryByRole("button", { name: /^Yakin\?/ })).not.toBeInTheDocument();
    // Keyboard: focus is on the row, so there is no time limit (WCAG 2.2.1); Escape or leaving cancels.
    await user.click(screen.getByRole("button", { name: "Kill Vitest" }));
    await screen.findByRole("button", { name: /^Yakin\?/ });
    act(() => void vi.advanceTimersByTime(60_000));
    expect(screen.getByRole("button", { name: /^Yakin\?/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("button", { name: /^Yakin\?/ })).not.toBeInTheDocument();
  });

  it("the second click confirms", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    planResult = plan({ needs_confirm: true });
    render(<ProcessTab data={data(many())} />);
    await user.click(screen.getByRole("button", { name: "Kill Vitest" }));
    const sure = await screen.findByRole("button", { name: /^Yakin\?/ });
    act(() => void vi.advanceTimersByTime(500));
    await user.click(sure);
    expect(calls("start_kill")).toEqual([
      ["start_kill", { targets: [{ pid: 21, start_time: 210 }, { pid: 22, start_time: 220 }], opId: "kill:test:Vitest:TOOLS-LAB", confirmed: true }],
    ]);
    expect(calls("plan_kill")).toHaveLength(1);
  });

  it("moving away reverts the confirmation", async () => {
    planResult = plan({ needs_confirm: true });
    render(<ProcessTab data={data(many())} />);
    await userEvent.click(screen.getByRole("button", { name: "Kill Vitest" }));
    const sure = await screen.findByRole("button", { name: /^Yakin\?/ });
    fireEvent.mouseLeave(sure.closest("li")!);
    expect(screen.queryByRole("button", { name: /^Yakin\?/ })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Kill Vitest" }));
    const again = await screen.findByRole("button", { name: /^Yakin\?/ });
    act(() => again.blur());
    expect(screen.queryByRole("button", { name: /^Yakin\?/ })).not.toBeInTheDocument();
  });

  it("the warning names what would die", async () => {
    planResult = plan({ needs_confirm: true, warnings: ["Sesi Claude Code di TOOLS-LAB ikut mati"] });
    render(<ProcessTab data={data(many())} />);
    await userEvent.click(screen.getByRole("button", { name: "Kill Vitest" }));
    expect(await screen.findByText("Sesi Claude Code di TOOLS-LAB ikut mati")).toBeInTheDocument();
  });

  it("snapshots keep a pending Yakin?, the search, the sort and expanded rows, and a vanished row just goes", async () => {
    planResult = plan({ needs_confirm: true, warnings: ["Sesi Claude Code di TOOLS-LAB ikut mati"] });
    const { rerender } = render(<ProcessTab data={data(many())} />);
    await userEvent.click(screen.getByRole("button", { name: "CPU" }));
    await userEvent.click(screen.getByRole("button", { name: /^Chrome/ }));
    await userEvent.click(screen.getByRole("button", { name: "Kill Vitest" }));
    await screen.findByRole("button", { name: /^Yakin\?/ });
    rerender(<ProcessTab data={data(many().map((g) => ({ ...g, footprint: g.footprint + 1 })))} />);
    expect(screen.getByRole("button", { name: /^Yakin\?/ })).toBeInTheDocument();
    expect(screen.getByText("Sesi Claude Code di TOOLS-LAB ikut mati")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "CPU" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("Chrome Helper")).toBeInTheDocument();
    rerender(<ProcessTab data={data(many().filter((g) => !g.id.startsWith("test:")))} />);
    expect(screen.queryByText("Vitest")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Yakin\?/ })).not.toBeInTheDocument();
  });

  it("keeps the search text across snapshots", async () => {
    const { rerender } = render(<ProcessTab data={data(many())} />);
    await userEvent.type(screen.getByRole("textbox", { name: "Cari proses" }), "vit");
    rerender(<ProcessTab data={data(many())} />);
    expect(screen.getByRole("textbox", { name: "Cari proses" })).toHaveValue("vit");
    expect(screen.queryByText("Chrome")).not.toBeInTheDocument();
  });

  it("shows Menghentikan… for an operation still running after reopen, on the right row only", () => {
    render(<ProcessTab data={data(many(), { operations: [op("kill:test:Vitest:TOOLS-LAB", "Running")] })} />);
    const row = screen.getByText("Vitest").closest("li")!;
    expect(within(row).getByText("Menghentikan…")).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /^Kill/ })).not.toBeInTheDocument();
    expect(screen.getAllByText("Menghentikan…")).toHaveLength(1);
  });

  it("shows Menghentikan… right after the click, before the next snapshot", async () => {
    render(<ProcessTab data={data(many())} />);
    await userEvent.click(screen.getByRole("button", { name: "Kill next dev" }));
    expect(await screen.findByText("Menghentikan…")).toBeInTheDocument();
  });

  it("survivors offer Paksa, and Paksa sends only the survivors with the same op id", async () => {
    const survivors = [{ pid: 21, start_time: 210 }];
    render(<ProcessTab data={data(many(), { operations: [op("kill:test:Vitest:TOOLS-LAB", "StillAlive", survivors)] })} />);
    await userEvent.click(screen.getByRole("button", { name: /^Paksa/ }));
    expect(calls("force_kill")).toEqual([["force_kill", { targets: survivors, opId: "kill:test:Vitest:TOOLS-LAB" }]]);
  });

  it("failure shows one line under the row, even with survivors", () => {
    const text = "Tidak bisa dihentikan: izin ditolak";
    render(
      <ProcessTab data={data(many(), { operations: [op("kill:test:Vitest:TOOLS-LAB", { Failed: text }, [{ pid: 21, start_time: 210 }])] })} />,
    );
    const row = screen.getByText("Vitest").closest("li")!;
    expect(within(row).getByText(text)).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /^Paksa/ })).toBeInTheDocument();
  });

  it("a refused start shows the reason under the row and returns the Kill button", async () => {
    invoke.mockImplementation((cmd: string) =>
      cmd === "start_kill" ? Promise.reject("Tidak bisa dihentikan: proses sistem") : Promise.resolve(plan()),
    );
    render(<ProcessTab data={data([group("g:7", "Solo")])} />);
    await userEvent.click(screen.getByRole("button", { name: "Kill Solo" }));
    expect(await screen.findByText("Tidak bisa dihentikan: proses sistem")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Kill Solo" })).toBeInTheDocument();
  });

  it("announces progress and results in a polite live region", async () => {
    const g = many();
    const { rerender } = render(<ProcessTab data={data(g)} />);
    const live = screen.getByRole("status");
    expect(live).toHaveAttribute("aria-live", "polite");
    rerender(<ProcessTab data={data(g, { operations: [op("kill:test:Vitest:TOOLS-LAB", "Running")] })} />);
    expect(live).toHaveTextContent("Menghentikan Vitest");
    rerender(<ProcessTab data={data(g, { operations: [op("kill:test:Vitest:TOOLS-LAB", { Failed: "izin ditolak" })] })} />);
    expect(live).toHaveTextContent("izin ditolak");
    rerender(<ProcessTab data={data(g, { operations: [op("kill:test:Vitest:TOOLS-LAB", { Done: { freed_bytes: null, skipped: 0 } })] })} />);
    expect(live).toHaveTextContent("Vitest dihentikan");
  });
});

describe("ProcessTab messages and labels", () => {
  const long = `Gagal menghentikan ${"/Users/example/Documents/proyek-yang-sangat-panjang/".repeat(3)}`;
  const failing = () =>
    invoke.mockImplementation((cmd: string) => (cmd === "plan_kill" ? Promise.resolve(plan()) : cmd === "start_kill" ? Promise.reject(long) : Promise.resolve(null)));

  it("a long failure wraps instead of being cut at the panel edge, under the text of a group row", async () => {
    failing();
    render(<ProcessTab data={data(many())} />);
    await userEvent.click(screen.getByRole("button", { name: "Kill Vitest" }));
    const line = await screen.findByText(long);
    expect(line).toHaveClass("[overflow-wrap:anywhere]", "pl-[62px]");
  });

  it("a search result without an icon has its message under its own text, not 62px in", async () => {
    failing();
    render(<ProcessTab data={data(many())} />);
    await userEvent.type(screen.getByRole("textbox", { name: "Cari proses" }), "worker 1");
    await userEvent.click(screen.getByRole("button", { name: "Kill worker 1" }));
    const line = await screen.findByText(long);
    expect(line).toHaveClass("pl-1.5");
    expect(line).not.toHaveClass("pl-[62px]");
  });

  it("a nested row's message lines up with its text", async () => {
    failing();
    render(<ProcessTab data={data(many())} />);
    await userEvent.click(screen.getByRole("button", { name: /^Vitest/ }));
    await userEvent.click(screen.getByRole("button", { name: "Kill worker 2" }));
    expect(await screen.findByText(long)).toHaveClass("pl-9");
  });

  it("the RAM and CPU switch is a labelled group", () => {
    render(<ProcessTab data={data(many())} />);
    const sort = screen.getByRole("group", { name: "Urutkan" });
    expect(within(sort).getByRole("button", { name: "RAM" })).toBeInTheDocument();
    expect(within(sort).getByRole("button", { name: "CPU" })).toBeInTheDocument();
  });

  it("a name that is cut off can still be read in full", () => {
    render(<ProcessTab data={data([group("dev:x", "next dev", { project: "proyek-contoh-yang-namanya-panjang", ports: [3000] })])} />);
    expect(screen.getByText("next dev")).toHaveAttribute("title", "next dev");
    const sub = screen.getByText(/1 proses/);
    expect(sub).toHaveAttribute("title", sub.textContent);
  });

  it("a leftover's long command can be read in full", async () => {
    const command = "node /Users/example/proyek/node_modules/.bin/vitest run --watch=false --reporter=verbose";
    const leftovers = [{ pid: 1, start_time: 10, command, project: "TOOLS-LAB", age_secs: 5400, kind: "test" as const, preselected: true, footprint: 1 }];
    render(<ProcessTab data={data([], { leftovers })} />);
    await userEvent.click(screen.getByRole("button", { name: "Bersihkan" }));
    expect(screen.getByText(command)).toHaveAttribute("title", command);
  });
});

describe("ProcessTab while CPU is not known yet", () => {
  it("shows … instead of 0% until the second read", () => {
    const { rerender } = render(<ProcessTab data={data(many(), { cpu_pending: true })} />);
    expect(screen.queryByText("70%")).not.toBeInTheDocument();
    expect(screen.queryByText("0%")).not.toBeInTheDocument();
    expect(screen.getAllByText("…").length).toBeGreaterThan(0);
    rerender(<ProcessTab data={data(many(), { cpu_pending: false })} />);
    expect(screen.getByText("70%")).toBeInTheDocument();
    expect(screen.queryByText("…")).not.toBeInTheDocument();
  });

  it("the CPU sort shows it as the main figure while pending", async () => {
    render(<ProcessTab data={data(many(), { cpu_pending: true })} />);
    await userEvent.click(screen.getByRole("button", { name: "CPU" }));
    expect(screen.getAllByText("…").length).toBe(3);
    expect(screen.getByText("3,0 GB")).toBeInTheDocument();
  });
});

describe("ProcessTab leftovers", () => {
  const left = (pid: number, command: string, o: Partial<Leftover> = {}): Leftover => ({
    pid,
    start_time: pid * 10,
    command,
    project: "TOOLS-LAB",
    age_secs: 5400,
    kind: "test",
    preselected: true,
    footprint: 100 * 1024 ** 2,
    ...o,
  });

  it("is hidden when empty", () => {
    render(<ProcessTab data={data(many())} />);
    expect(screen.queryByText(/proses sisa/)).not.toBeInTheDocument();
  });

  it("starts with test runners checked, dev tools unchecked, and the button follows the checks", async () => {
    const leftovers = [left(1, "vitest run"), left(2, "jest --watch"), left(3, "next dev", { kind: "dev", preselected: false })];
    render(<ProcessTab data={data(many(), { leftovers })} />);
    expect(screen.getByText("3 proses sisa")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Bersihkan" }));
    expect(screen.getByRole("checkbox", { name: /vitest run/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /jest --watch/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /next dev/ })).not.toBeChecked();
    expect(screen.getAllByText(/TOOLS-LAB/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/1 jam/).length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Hentikan 2 proses" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: /next dev/ }));
    expect(screen.getByRole("button", { name: "Hentikan 3 proses" })).toBeInTheDocument();
  });

  it("stops the checked ones through start_kill with confirmed true, without asking again", async () => {
    const leftovers = [left(1, "vitest run"), left(3, "next dev", { kind: "dev", preselected: false })];
    render(<ProcessTab data={data([], { leftovers })} />);
    await userEvent.click(screen.getByRole("button", { name: "Bersihkan" }));
    await userEvent.click(screen.getByRole("button", { name: "Hentikan 1 proses" }));
    expect(calls("plan_kill")).toHaveLength(0);
    expect(calls("start_kill")).toEqual([["start_kill", { targets: [{ pid: 1, start_time: 10 }], opId: "kill:leftovers", confirmed: true }]]);
  });

  it("keeps checks across snapshots", async () => {
    const leftovers = [left(1, "vitest run"), left(3, "next dev", { kind: "dev", preselected: false })];
    const { rerender } = render(<ProcessTab data={data([], { leftovers })} />);
    await userEvent.click(screen.getByRole("button", { name: "Bersihkan" }));
    await userEvent.click(screen.getByRole("checkbox", { name: /next dev/ }));
    rerender(<ProcessTab data={data([], { leftovers: leftovers.map((l) => ({ ...l })) })} />);
    expect(screen.getByRole("checkbox", { name: /next dev/ })).toBeChecked();
  });
});

describe("ProcessTab keyboard", () => {
  it("slash focuses search, arrows move rows, Enter expands", async () => {
    const user = userEvent.setup();
    render(<ProcessTab data={data(many())} />);
    await user.keyboard("/");
    const box = screen.getByRole("textbox", { name: "Cari proses" });
    expect(box).toHaveFocus();
    expect(box).toHaveValue("");
    await user.keyboard("{ArrowDown}");
    const heads = screen.getAllByRole("button", { name: /\d+ proses|sesi|:\d+/ });
    expect(rows()).toHaveLength(3);
    expect(heads[0]).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(heads[1]).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(heads[0]).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(heads[0]).toHaveAttribute("aria-expanded", "true");
    await user.keyboard("{Enter}");
    expect(heads[0]).toHaveAttribute("aria-expanded", "false");
  });

  it("Escape cancels Yakin? first, then clears the search, and only then reaches the panel", async () => {
    const user = userEvent.setup();
    const onWindow = vi.fn((e: KeyboardEvent) => e.key === "Escape" && e.defaultPrevented);
    window.addEventListener("keydown", onWindow);
    planResult = plan({ needs_confirm: true });
    render(<ProcessTab data={data(many())} />);
    await user.type(screen.getByRole("textbox", { name: "Cari proses" }), "vitest");
    await user.click(screen.getByRole("button", { name: "Kill Vitest" }));
    await screen.findByRole("button", { name: /^Yakin\?/ });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("button", { name: /^Yakin\?/ })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Cari proses" })).toHaveValue("vitest");
    await user.keyboard("{Escape}");
    expect(screen.getByRole("textbox", { name: "Cari proses" })).toHaveValue("");
    await user.keyboard("{Escape}");
    window.removeEventListener("keydown", onWindow);
    expect(onWindow.mock.results.map((r) => r.value).slice(-3)).toEqual([true, true, false]);
  });
});

describe("ProcessTab fix round 1", () => {
  const pair = (a: number, b: number) => [group("a:1", "Alpha", { footprint: a * GB }), group("b:2", "Beta", { footprint: b * GB })];
  const order = () => screen.getAllByRole("button", { name: /^(Alpha|Beta|Gamma)/ }).map((b) => /^(Alpha|Beta|Gamma)/.exec(b.textContent ?? "")?.[0]);

  it("holds the row order while the pointer is on the list, so a hidden Kill never moves under it", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<ProcessTab data={data(pair(2, 1.9))} />);
    await user.hover(screen.getByRole("button", { name: /^Alpha/ }));
    rerender(<ProcessTab data={data(pair(1.9, 3))} />);
    expect(order()).toEqual(["Alpha", "Beta"]);
    expect(screen.getByText("3,0 GB")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Kill Alpha" }));
    expect(calls("start_kill")[0][1]).toMatchObject({ targets: [{ pid: 1, start_time: 10 }] });
    await user.unhover(screen.getByRole("button", { name: /^Alpha/ }));
    await user.unhover(screen.getByRole("button", { name: /^Alpha/ }));
    fireEvent.mouseLeave(screen.getByRole("button", { name: /^Alpha/ }).closest("ul")!);
    expect(order()).toEqual(["Beta", "Alpha"]);
  });

  it("holds the order while a row has focus; new rows go last and removed rows go away", () => {
    const { rerender } = render(<ProcessTab data={data(pair(2, 1.9))} />);
    act(() => screen.getByRole("button", { name: /^Alpha/ }).focus());
    rerender(<ProcessTab data={data([...pair(1.9, 3), group("c:3", "Gamma", { footprint: 9 * GB })])} />);
    expect(order()).toEqual(["Alpha", "Beta", "Gamma"]);
    rerender(<ProcessTab data={data([pair(1.9, 3)[1]])} />);
    expect(order()).toEqual(["Beta"]);
  });

  it("ignores a confirming press for 400 ms after Yakin? appears", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    planResult = plan({ needs_confirm: true });
    render(<ProcessTab data={data(many())} />);
    await user.click(screen.getByRole("button", { name: "Kill Vitest" }));
    const sure = await screen.findByRole("button", { name: /^Yakin\?/ });
    act(() => void vi.advanceTimersByTime(100));
    await user.click(sure);
    expect(calls("start_kill")).toHaveLength(0);
    act(() => void vi.advanceTimersByTime(400));
    await user.click(screen.getByRole("button", { name: /^Yakin\?/ }));
    expect(calls("start_kill")).toHaveLength(1);
  });

  const askVitest = async (user: ReturnType<typeof userEvent.setup>) => {
    planResult = plan({ needs_confirm: true, warnings: ["w"] });
    const view = render(<ProcessTab data={data(many())} />);
    await user.click(screen.getByRole("button", { name: "Kill Vitest" }));
    await screen.findByRole("button", { name: /^Yakin\?/ });
    act(() => void vi.advanceTimersByTime(500));
    return view;
  };
  const withChild = (extra: boolean) =>
    many().map((g) => (g.id.startsWith("test:") && extra ? { ...g, processes: [...g.processes, proc(23, "worker 3")] } : g));

  it("kills exactly the targets that were confirmed, even if the group lost one", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const { rerender } = await askVitest(user);
    rerender(<ProcessTab data={data(many().map((g) => (g.id.startsWith("test:") ? { ...g, processes: g.processes.slice(0, 1) } : g)))} />);
    await user.click(screen.getByRole("button", { name: /^Yakin\?/ }));
    expect(calls("start_kill")[0][1].targets).toEqual([{ pid: 21, start_time: 210 }, { pid: 22, start_time: 220 }]);
  });

  it("drops the confirmation and plans again when the group gained a process", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const { rerender } = await askVitest(user);
    rerender(<ProcessTab data={data(withChild(true))} />);
    await user.click(screen.getByRole("button", { name: /^Yakin\?/ }));
    expect(calls("start_kill")).toHaveLength(0);
    expect(calls("plan_kill")).toHaveLength(2);
    expect(calls("plan_kill")[1][1].targets).toHaveLength(3);
  });

  it("keeps keyboard focus on the row's button when it turns into Menghentikan…", async () => {
    const user = userEvent.setup();
    render(<ProcessTab data={data([group("g:7", "Solo")])} />);
    const kill = screen.getByRole("button", { name: "Kill Solo" });
    kill.focus();
    await user.keyboard("{Enter}");
    await screen.findByText("Menghentikan…");
    expect(document.activeElement).toHaveTextContent("Menghentikan…");
    expect(document.activeElement).toHaveAttribute("aria-disabled", "true");
  });

  it("a newer Failed replaces a stale local error", async () => {
    invoke.mockImplementation((cmd: string) => (cmd === "start_kill" ? Promise.reject("stale") : Promise.resolve(plan())));
    const g = [group("g:7", "Solo")];
    const { rerender } = render(<ProcessTab data={data(g)} />);
    await userEvent.click(screen.getByRole("button", { name: "Kill Solo" }));
    expect(await screen.findByText("stale")).toBeInTheDocument();
    rerender(<ProcessTab data={data(g, { operations: [op("kill:g:7", { Failed: "izin ditolak" })] })} />);
    const row = screen.getByText("Solo").closest("li")!;
    expect(within(row).getByText("izin ditolak")).toBeInTheDocument();
    expect(screen.queryByText("stale")).not.toBeInTheDocument();
  });

  it("a plan_kill rejection shows the reason and keeps Kill", async () => {
    planResult = new Error("rencana gagal");
    render(<ProcessTab data={data([group("g:7", "Solo")])} />);
    await userEvent.click(screen.getByRole("button", { name: "Kill Solo" }));
    expect(await screen.findByText("rencana gagal")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Kill Solo" })).toBeInTheDocument();
    expect(calls("start_kill")).toHaveLength(0);
  });

  it("ties the warning to Yakin? and announces it", async () => {
    const user = userEvent.setup();
    planResult = plan({ needs_confirm: true, warnings: ["Sesi Claude Code di TOOLS-LAB ikut mati"] });
    render(<ProcessTab data={data(many())} />);
    await user.click(screen.getByRole("button", { name: "Kill Vitest" }));
    const sure = await screen.findByRole("button", { name: /^Yakin\?/ });
    expect(sure).toHaveAccessibleDescription("Sesi Claude Code di TOOLS-LAB ikut mati");
    expect(screen.getByRole("status")).toHaveTextContent(/Yakin\?.*Vitest.*Sesi Claude Code di TOOLS-LAB ikut mati/);
  });

  it("a leftover that appears while the list is open starts unchecked, and vanished ones are pruned", async () => {
    const l = (pid: number, command: string): Leftover => ({ pid, start_time: pid * 10, command, project: null, age_secs: 60, kind: "test", preselected: true, footprint: null });
    const { rerender } = render(<ProcessTab data={data([], { leftovers: [l(1, "vitest run")] })} />);
    await userEvent.click(screen.getByRole("button", { name: "Bersihkan" }));
    rerender(<ProcessTab data={data([], { leftovers: [l(1, "vitest run"), l(2, "jest")] })} />);
    expect(screen.getByRole("checkbox", { name: /jest/ })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Hentikan 1 proses" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: /vitest run/ }));
    rerender(<ProcessTab data={data([], { leftovers: [l(2, "jest")] })} />);
    rerender(<ProcessTab data={data([], { leftovers: [l(1, "vitest run"), l(2, "jest")] })} />);
    expect(screen.getByRole("checkbox", { name: /vitest run/ })).not.toBeChecked();
  });
});
