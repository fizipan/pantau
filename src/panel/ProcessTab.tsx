import { useEffect, useMemo, useRef, useState } from "react";
import { Search } from "lucide-react";
import type { Operation, PanelData, ProcId, ProcessGroup, ProcessRow as Proc } from "@/lib/api";
import { formatBytes } from "@/lib/format";
import type { Confirm } from "./KillButton";
import { LEFTOVER_OP, LeftoverCard } from "./LeftoverCard";
import { ProcessRow } from "./ProcessRow";

type Sort = "ram" | "cpu";
type Entry = { group: ProcessGroup; proc?: Proc };

const TOP = 8;
/** Long enough to read the question; a keyboard or screen reader user on the row is not timed at all. */
const CONFIRM_MS = 8000;
const isReadable = (p: Proc) => p.footprint !== null;
const ids = (ps: Proc[]) => ps.filter((p) => p.killable).map(({ pid, start_time }) => ({ pid, start_time }));
const ports = (p: number[]) => p.map((n) => `:${n}`);

/** Biggest first; a process macOS would not read sorts last. */
const weight = (sort: Sort, e: { footprint: number | null; cpu_pct: number }) => (sort === "ram" ? (e.footprint ?? -1) : e.cpu_pct);

/** Whether focus is inside the row (or a row nested in it) that `opId` names. */
function focusIsIn(opId: string) {
  for (let el = document.activeElement; el; el = el.parentElement) if (el.getAttribute("data-opid") === opId) return true;
  return false;
}

function amounts(sort: Sort, readable: boolean, footprint: number | null, cpu: number, cpuPending: boolean) {
  if (!readable || footprint === null) return { primary: null, secondary: null };
  const ram = formatBytes(footprint);
  const pct = cpuPending ? "…" : `${Math.round(cpu)}%`;
  return sort === "ram" ? { primary: ram, secondary: pct } : { primary: pct, secondary: ram };
}

function groupSubline(g: ProcessGroup) {
  const count = g.kind === "agent" && g.session_count > 0 ? `${g.session_count} sesi` : `${g.processes.length} proses`;
  return [count, g.project, ports(g.ports).join(" ")].filter(Boolean).join(" · ");
}

/** The list: biggest first, the top 8 groups; search reaches every process, readable or not. */
function entries(groups: ProcessGroup[], sort: Sort, needle: string): Entry[] {
  const value = (e: Entry) => (e.proc ? weight(sort, e.proc) : weight(sort, e.group));
  if (!needle) {
    return groups
      .filter((g) => g.processes.some(isReadable))
      .map((group) => ({ group }))
      .sort((a, b) => value(b) - value(a))
      .slice(0, TOP);
  }
  const found: Entry[] = [];
  for (const group of groups) {
    if ([group.name, group.project ?? ""].some((s) => s.toLowerCase().includes(needle))) found.push({ group });
    else for (const proc of group.processes) if ([proc.label, ...ports(proc.ports)].some((s) => s.toLowerCase().includes(needle))) found.push({ group, proc });
  }
  return found.sort((a, b) => value(b) - value(a));
}

const entryKey = (e: Entry) => (e.proc ? `${e.proc.pid}:${e.proc.start_time}` : e.group.id);

/** Text for the polite live region: what changed in a kill since the last snapshot. */
function useAnnouncements(data: PanelData) {
  const names = useRef(new Map<string, string>());
  const seen = useRef<Map<string, string> | null>(null);
  const [text, say] = useState("");
  for (const g of data.groups) {
    names.current.set(`kill:${g.id}`, g.name);
    for (const p of g.processes) names.current.set(`kill:${p.pid}`, p.label);
  }
  useEffect(() => {
    const now = new Map<string, string>();
    const said: string[] = [];
    for (const o of data.operations) {
      if (!o.op_id.startsWith("kill:")) continue;
      const state = typeof o.state === "string" ? o.state : "Done" in o.state ? "Done" : `Failed:${o.state.Failed}`;
      now.set(o.op_id, state);
      // The first snapshot after opening is history, not news.
      if (!seen.current || seen.current.get(o.op_id) === state) continue;
      const name = o.op_id === LEFTOVER_OP ? "Proses sisa" : (names.current.get(o.op_id) ?? "Proses");
      said.push(
        state === "Running" ? `Menghentikan ${name}` : state === "Done" ? `${name} dihentikan` : state === "StillAlive" ? `${name} masih berjalan` : state.slice("Failed:".length),
      );
    }
    seen.current = now;
    if (said.length) say(said.join(". "));
  }, [data.operations]);
  return { text, say, nameOf: (opId: string) => names.current.get(opId) ?? "Proses" };
}

export function ProcessTab({ data }: { data: PanelData }) {
  const [sort, setSort] = useState<Sort>("ram");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const [pending, setPending] = useState<{ id: string; warnings: string[]; targets: ProcId[] } | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const announce = useAnnouncements(data);
  // While the pointer or focus is on the list, rows keep their order (values still update), so a
  // click never lands on a row that moved. `ctx` ties the hold to the sort and search it was taken under.
  const [held, setHeld] = useState<{ ctx: string; keys: string[] } | null>(null);
  const inside = useRef({ pointer: false, focus: false });
  const shown = useRef<string[]>([]);

  const confirm: Confirm = {
    id: pending?.id ?? null,
    warnings: pending?.warnings ?? [],
    targets: pending?.targets ?? [],
    ask: (id, warnings, targets) => {
      setPending({ id, warnings, targets });
      announce.say(`Yakin? ${announce.nameOf(id)}${warnings.map((w) => `. ${w}`).join("")}`);
    },
    cancel: () => setPending(null),
  };
  useEffect(() => {
    if (!pending) return;
    // Focus on the row means someone may still be deciding (WCAG 2.2.1): leaving the row or Escape cancels instead.
    const t = setTimeout(() => focusIsIn(pending.id) || setPending(null), CONFIRM_MS);
    return () => clearTimeout(t);
  }, [pending]);

  // Capture phase: Escape reaches here before the panel's own handler, which then sees `defaultPrevented`.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const active = document.activeElement;
      const typing = active === search.current;
      if (e.key === "Escape") {
        if (pending) setPending(null);
        else if (query) setQuery("");
        else return;
        e.preventDefault();
      } else if (e.key === "/" && !typing) {
        e.preventDefault();
        search.current?.focus();
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        const navs = [...(root.current?.querySelectorAll<HTMLElement>("[data-nav]") ?? [])];
        let at: number;
        if (typing) {
          if (e.key === "ArrowUp") return;
          at = -1;
        } else {
          const row = active?.closest("[data-rowitem]");
          if (!row || !root.current?.contains(row)) return;
          at = navs.findIndex((n) => n.closest("[data-rowitem]") === row);
        }
        e.preventDefault();
        navs[Math.min(Math.max(at + (e.key === "ArrowDown" ? 1 : -1), 0), navs.length - 1)]?.focus();
      } else if (e.key.length === 1 && e.key !== " " && !typing) {
        // Typing anywhere opens the search; the key lands in the field once it has focus.
        search.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [pending, query]);

  const needle = query.trim().toLowerCase();
  const ctx = `${sort}|${needle}`;
  const fresh = useMemo(() => entries(data.groups, sort, needle), [data.groups, sort, needle]);
  const list = useMemo(() => {
    if (!held || held.ctx !== ctx) return fresh;
    const at = (e: Entry) => {
      const i = held.keys.indexOf(entryKey(e));
      return i < 0 ? Infinity : i;
    };
    // Stable: rows new to the list keep their sorted order, after everything held.
    return [...fresh].sort((a, b) => at(a) - at(b));
  }, [fresh, held, ctx]);
  shown.current = list.map(entryKey);
  const hold = (part: Partial<typeof inside.current>) => {
    Object.assign(inside.current, part);
    setHeld(inside.current.pointer || inside.current.focus ? (h) => h ?? { ctx, keys: shown.current } : null);
  };
  const operations = useMemo(() => new Map<string, Operation>(data.operations.map((o) => [o.op_id, o])), [data.operations]);
  const toggle = (id: string) => {
    const next = new Set(open);
    if (!next.delete(id)) next.add(id);
    setOpen(next);
  };

  const procRow = (p: Proc, subline: string, nested: boolean) => {
    const opId = `kill:${p.pid}`;
    return (
      <ProcessRow
        key={`${p.pid}:${p.start_time}`}
        nested={nested}
        title={p.label}
        subline={subline}
        {...amounts(sort, isReadable(p), p.footprint, p.cpu_pct, data.cpu_pending)}
        icon={null}
        targets={ids([p])}
        opId={opId}
        operation={operations.get(opId)}
        confirm={confirm}
      />
    );
  };

  return (
    <div ref={root}>
      <div className="mb-1 flex items-center gap-2 px-1.5">
        <span className="text-[12.5px] font-medium text-muted">Paling boros</span>
        <label className="relative min-w-0 flex-1">
          <Search aria-hidden="true" strokeWidth={2} className="pointer-events-none absolute top-1/2 left-2 size-3 -translate-y-1/2 text-muted" />
          <input
            ref={search}
            type="text"
            aria-label="Cari proses"
            placeholder="Cari"
            autoComplete="off"
            spellCheck={false}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="h-[26px] w-full rounded-md bg-seg-track pr-2 pl-6 text-[12.5px] text-ink select-text placeholder:text-muted"
          />
        </label>
        <div role="group" aria-label="Urutkan" className="flex rounded-md bg-seg-track p-0.5">
          {(["ram", "cpu"] as const).map((s) => (
            <button
              key={s}
              type="button"
              aria-pressed={sort === s}
              className="rounded-[5px] px-2.5 py-[3px] text-[12px] font-medium text-muted aria-pressed:bg-seg-on aria-pressed:text-ink aria-pressed:shadow-seg motion-safe:transition-colors motion-safe:duration-150"
              onClick={() => setSort(s)}
            >
              {s === "ram" ? "RAM" : "CPU"}
            </button>
          ))}
        </div>
      </div>
      <ul
        onMouseEnter={() => hold({ pointer: true })}
        onMouseLeave={() => hold({ pointer: false })}
        onFocus={() => hold({ focus: true })}
        onBlur={(e) => e.currentTarget.contains(e.relatedTarget) || hold({ focus: false })}
      >
        {list.map(({ group, proc }) => {
          if (proc) return procRow(proc, [group.name, ...ports(proc.ports)].join(" · "), false);
          const opId = `kill:${group.id}`;
          const readable = group.processes.some(isReadable);
          const expanded = open.has(group.id);
          return (
            <ProcessRow
              key={group.id}
              title={group.name}
              subline={groupSubline(group)}
              {...amounts(sort, readable, group.footprint, group.cpu_pct, data.cpu_pending)}
              icon={{ kind: group.kind, bundle: group.bundle }}
              targets={ids(group.processes)}
              opId={opId}
              operation={operations.get(opId)}
              confirm={confirm}
              expanded={expanded}
              onToggle={() => toggle(group.id)}
            >
              {expanded && (
                <ul>
                  {group.processes
                    // Root and other users' processes stay out of the list, but a search reaches them.
                    .filter((p) => needle || isReadable(p))
                    .sort((a, b) => weight(sort, b) - weight(sort, a))
                    .map((p) => procRow(p, ports(p.ports).join(" "), true))}
                </ul>
              )}
            </ProcessRow>
          );
        })}
      </ul>
      {data.groups.length === 0 ? (
        <p className="px-1.5 py-2 text-[12.5px] text-muted">Memuat proses…</p>
      ) : (
        needle && list.length === 0 && <p className="px-1.5 py-2 text-[12.5px] text-muted">Tidak ada proses yang cocok</p>
      )}
      <LeftoverCard leftovers={data.leftovers} operation={operations.get(LEFTOVER_OP)} />
      <div role="status" aria-live="polite" className="sr-only">
        {announce.text}
      </div>
    </div>
  );
}
