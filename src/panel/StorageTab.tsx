import { useEffect, useRef, useState } from "react";
import { ChevronRight, RefreshCw } from "lucide-react";
import { getStorage, refreshStorage, scanRecentNow, type Operation, type StorageData, type StorageItem } from "@/lib/api";
import { formatAge } from "@/lib/format";
import { RecentList } from "./RecentList";
import { Heading, PermissionLine, StorageRow, deniedFolder, outcome, type StorageConfirm } from "./StorageRow";

/** Sizes arrive item by item while Rust measures, so the tab asks again every few seconds. */
const POLL_MS = 3000;
/** The scheduler remeasures every 10 minutes; older than that means it has not managed to. */
const STALE_MS = 10 * 60_000;
const MODULES = "node-modules-";

const projectName = (path: string) => path.split("/").filter(Boolean).pop() ?? path;

/** Text for the polite live region: what a clean did since the last snapshot. */
function useAnnouncements(operations: Operation[], items: StorageItem[]) {
  const seen = useRef<Map<string, string> | null>(null);
  const [text, say] = useState("");
  const titles = useRef(new Map<string, string>());
  for (const i of items) titles.current.set(`clean:${i.id}`, i.title);
  useEffect(() => {
    const now = new Map<string, string>();
    const said: string[] = [];
    for (const o of operations) {
      if (!o.op_id.startsWith("clean:")) continue;
      const s = o.state;
      const state = typeof s === "string" ? s : "Done" in s ? `Done:${s.Done.freed_bytes}:${s.Done.skipped}` : `Failed:${s.Failed}`;
      now.set(o.op_id, state);
      // The first snapshot after opening is history, not news.
      if (!seen.current || seen.current.get(o.op_id) === state) continue;
      const name = titles.current.get(o.op_id) ?? "Item";
      said.push(typeof s === "string" ? (s === "Running" ? `Membersihkan ${name}` : name) : "Done" in s ? `${name}: ${outcome(o)}` : s.Failed);
    }
    seen.current = now;
    if (said.length) say(said.join(". "));
  }, [operations]);
  return { text, say };
}

/** The id of a `.next` item is `next-<path>`; anything else falls back to what the tab last knew, or the id. */
function GoneRow({ op }: { op: Operation }) {
  const id = op.op_id.slice("clean:".length);
  const isNext = id.startsWith("next-");
  return (
    <li className="py-1.5 pr-1.5 pl-1.5">
      <p className="truncate text-[13px] font-[550]">{isNext ? "Cache build Next.js" : id}</p>
      {isNext && (
        <p className="truncate text-[11px] text-subtle" title={id.slice(5)}>
          {id.slice(5)}
        </p>
      )}
      <p className="text-[11px] text-muted">{outcome(op)}</p>
    </li>
  );
}

export function StorageTab({ operations }: { operations: Operation[] }) {
  const [data, setData] = useState<StorageData | null>(null);
  const [failed, setFailed] = useState(false);
  const [reload, setReload] = useState(0);
  const [pending, setPending] = useState<{ id: string; bytes: number | null } | null>(null);
  const [open, setOpen] = useState(false);
  const announce = useAnnouncements(operations, data?.items ?? []);

  useEffect(() => {
    let live = true;
    let latest = 0;
    // An answer that arrives after a newer one was asked for is already out of date.
    const load = () => {
      const mine = ++latest;
      return getStorage().then(
        (next) => live && mine === latest && (setData(next), setFailed(false)),
        () => live && mine === latest && setFailed(true),
      );
    };
    void load();
    const timer = setInterval(load, POLL_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [reload]);

  const items = data?.items ?? [];
  const asked = items.find((i) => i.id === pending?.id);
  // A row that turned refused, or went away, no longer has a question to answer.
  useEffect(() => {
    if (pending && asked?.state !== "ready" && asked?.state !== "pending") setPending(null);
  }, [pending, asked]);

  // Capture phase: Escape reaches here before the panel's own handler, which then sees `defaultPrevented`.
  useEffect(() => {
    if (!pending) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.metaKey || e.ctrlKey || e.altKey) return;
      setPending(null);
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [pending]);

  const confirm: StorageConfirm = {
    id: pending?.id ?? null,
    bytes: pending?.bytes ?? null,
    ask: (id, bytes) => {
      setPending({ id, bytes });
      announce.say(`Yakin? Bersihkan ${items.find((i) => i.id === id)?.title ?? "item"}`);
    },
    cancel: () => setPending(null),
  };
  const ops = new Map(operations.map((o) => [o.op_id, o]));
  const row = (i: StorageItem, nested = false) => (
    <StorageRow key={i.id} item={nested ? { ...i, title: projectName(i.sub), sub: i.sub } : i} operation={ops.get(`clean:${i.id}`)} confirm={confirm} nested={nested} measuring={data?.measuring} />
  );

  if (!data) return failed ? <p className="px-1.5 text-[12.5px] text-muted">Data penyimpanan belum bisa dibaca</p> : null;

  const now = Date.now();
  const safe = items.filter((i) => i.category === "safe");
  const modules = items.filter((i) => i.id.startsWith(MODULES));
  const check = items.filter((i) => i.category === "check" && !i.id.startsWith(MODULES));
  // A removed folder leaves the list, but what its clean did must stay readable (Rust keeps the operation 60 s).
  const listed = new Set(items.map((i) => `clean:${i.id}`));
  const gone = operations.filter((o) => o.op_id.startsWith("clean:") && o.state !== "Running" && o.state !== "StillAlive" && !listed.has(o.op_id));
  const oldest = items.reduce<number | null>((m, i) => (i.measuredAtMs === null ? m : Math.min(m ?? Infinity, i.measuredAtMs)), null);
  const stale = oldest !== null && now - oldest > STALE_MS;
  const measureNow = () => {
    void refreshStorage().then(() => setReload((n) => n + 1));
    if (data.paused) void scanRecentNow();
  };

  return (
    <div>
      {data.paused && (
        <div className="mx-1.5 mb-2 flex items-center gap-2 rounded-lg bg-hover px-3 py-2">
          <p className="min-w-0 flex-1 text-[12.5px]">Dijeda karena memori penuh</p>
          <button type="button" className="rounded-md bg-seg-on px-2.5 py-1 text-[12.5px] font-medium shadow-seg" onClick={measureNow}>
            Ukur sekarang
          </button>
        </div>
      )}
      {stale && (
        <div className="mb-1 flex items-center justify-between gap-2 px-1.5 text-[11px] text-subtle">
          <span>
            Diukur {formatAge(oldest, now)}
            {data.measuring ? " · Mengukur…" : ""}
          </span>
          <button type="button" aria-label="Ukur ulang" disabled={data.measuring && !data.measuringAuto} className="grid size-5 place-items-center rounded-md text-muted hover:text-ink disabled:opacity-50" onClick={measureNow}>
            <RefreshCw aria-hidden="true" strokeWidth={2} className={`size-3 ${data.measuring ? "motion-safe:animate-spin" : ""}`} />
          </button>
        </div>
      )}
      {data.deniedRoots.map((root) => (
        <PermissionLine key={root} text={`Butuh izin untuk membaca ${deniedFolder(root)}`} className="pl-1.5" />
      ))}
      {(safe.length > 0 || gone.length > 0) && (
        <section aria-labelledby="safe-head">
          <Heading id="safe-head" aside={<span className="text-[11px] text-subtle">dibuat ulang saat perlu</span>}>
            Aman dibersihkan
          </Heading>
          <ul>
            {safe.map((i) => row(i))}
            {gone.map((o) => (
              <GoneRow key={o.op_id} op={o} />
            ))}
          </ul>
        </section>
      )}
      {(check.length > 0 || modules.length > 0) && (
        <section aria-labelledby="check-head" className="mt-3">
          <Heading id="check-head">Perlu dicek dulu</Heading>
          <ul>
            {check.map((i) => row(i))}
            {modules.length > 0 && (
              <li>
                <button
                  type="button"
                  aria-expanded={open}
                  className="group/row flex w-full items-center gap-2 rounded-lg py-1.5 pr-1.5 pl-1.5 text-left hover:bg-hover motion-safe:transition-colors motion-safe:duration-150"
                  onClick={() => setOpen(!open)}
                >
                  <ChevronRight aria-hidden="true" strokeWidth={2} className={`size-3 shrink-0 text-muted motion-safe:transition-transform motion-safe:duration-150 ${open ? "rotate-90" : ""}`} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13px] font-[550]">node_modules</span>
                    <span className="block truncate text-[11px] text-subtle">Ukuran dilihat per folder</span>
                  </span>
                  <span className="shrink-0 text-[13px]">{modules.length} folder</span>
                </button>
                {open && <ul>{modules.map((i) => row(i, true))}</ul>}
              </li>
            )}
          </ul>
        </section>
      )}
      <RecentList recent={data.recent} recentAtMs={data.recentAtMs} skippedDirs={data.recentSkippedDirs} denied={data.recentDenied} scanning={data.scanning} scanningAuto={data.scanningAuto} now={now} />
      <div role="status" aria-live="polite" className="sr-only">
        {announce.text}
      </div>
    </div>
  );
}
