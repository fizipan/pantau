import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { cleanItem, openPrivacySettings, revealInFinder, type Operation, type StorageItem } from "@/lib/api";
import { formatBytes } from "@/lib/format";

/** One pending question for the whole list, owned by `StorageTab` so Escape and a snapshot can reach it. */
export interface StorageConfirm {
  id: string | null;
  /** The size shown when the question was asked: a newer measurement must not change what was agreed to. */
  bytes: number | null;
  ask: (id: string, bytes: number | null) => void;
  cancel: () => void;
}

export const NO_STORAGE_CONFIRM: StorageConfirm = { id: null, bytes: null, ask: () => {}, cancel: () => {} };

/** A second press this soon after the question appears is a double click or key repeat, not a decision. */
const CONFIRM_DELAY_MS = 400;

const TEMP_RULE =
  "Hanya file milikmu yang tidak berubah 3 hari dan tidak sedang dibuka. Pantau tidak bisa melihat semua file yang terbuka di macOS.";

const message = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/** The folder macOS asks permission for: Desktop, Documents or Downloads when the path is under one. */
export function deniedFolder(path: string) {
  return /\/(Desktop|Documents|Downloads)(\/|$)/.exec(path)?.[1] ?? path.split("/").filter(Boolean).pop() ?? path;
}

const PILL = "rounded-md px-2 py-0.5 text-[11.5px] font-medium motion-safe:transition-opacity motion-safe:duration-150";
const NEUTRAL = `${PILL} bg-seg-on shadow-seg`;

export function Heading({ id, children, aside }: { id: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="mb-1 flex items-center justify-between gap-2 px-1.5">
      <h2 id={id} className="text-[12.5px] font-medium text-muted">
        {children}
      </h2>
      {aside}
    </div>
  );
}

/** For a folder macOS would not let Pantau read: says so, instead of showing a size of zero. */
export function PermissionLine({ text, className = "" }: { text: string; className?: string }) {
  return (
    <div className={`flex items-center gap-2 pr-1.5 pb-1 text-[11px] text-muted ${className}`}>
      <p className="min-w-0 flex-1">{text}</p>
      <button type="button" className={`${NEUTRAL} shrink-0`} onClick={() => void openPrivacySettings()}>
        Buka Pengaturan Sistem
      </button>
    </div>
  );
}

/** What a finished clean says: what came back, and how many entries were left alone. */
export function outcome(op: Operation | undefined) {
  const s = op?.state;
  if (!s || s === "Running" || s === "StillAlive") return null;
  if ("Failed" in s) return s.Failed;
  const back = s.Done.freed_bytes === null ? "Selesai" : `${formatBytes(s.Done.freed_bytes)} kembali`;
  return s.Done.skipped > 0 ? `${back} · ${s.Done.skipped} item dilewati` : back;
}

/** Holding Enter or Space repeats keydown; a repeat is never a decision, so it must not click. */
export const ignoreRepeat = (e: KeyboardEvent<HTMLElement>) => {
  if (e.repeat && (e.key === "Enter" || e.key === " ")) e.preventDefault();
};

export function StorageRow({ item, operation, confirm, nested, measuring }: {
  item: StorageItem;
  operation?: Operation;
  /** Rust is measuring now: a row whose clean just finished shows that instead of its old size. */
  measuring?: boolean;
  confirm: StorageConfirm;
  nested?: boolean;
}) {
  // Bridges the gap between the click and the first snapshot that carries the operation.
  const [started, setStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const askedAt = useRef(0);
  const noteId = useId();
  // Compared by value: every snapshot re-delivers an old result as a new object, which is not news.
  const stateKey = operation ? JSON.stringify(operation.state) : "";
  useEffect(() => {
    setStarted(false);
    setError(null);
  }, [stateKey]);
  // Backstop for a second clean that ends with exactly the same result as the first.
  useEffect(() => {
    if (!started) return;
    const t = setTimeout(() => setStarted(false), 15_000);
    return () => clearTimeout(t);
  }, [started]);

  const { id, title, sub, path, state } = item;
  const name = `${title}, ${sub}`;
  const denied = state === "denied";
  const refused = typeof state === "object" ? state.refused : null;
  const cleanable = item.cleaner !== "none";
  const busy = started || operation?.state === "Running";
  const asking = confirm.id === id && !busy;
  const line = error ?? (busy ? null : outcome(operation));
  const remeasuring = !!measuring && !busy && typeof operation?.state === "object" && "Done" in operation.state;

  const amount = denied ? null : item.bytes === null || remeasuring ? (measuring ? "Mengukur…" : "Belum diukur") : `${item.upperBound ? "hingga " : ""}${formatBytes(item.bytes)}`;

  const press = async () => {
    if (!asking) {
      askedAt.current = Date.now();
      confirm.ask(id, item.bytes);
      return;
    }
    if (Date.now() - askedAt.current < CONFIRM_DELAY_MS) return;
    confirm.cancel();
    setError(null);
    setStarted(true);
    try {
      await cleanItem(id, `clean:${id}`);
    } catch (e) {
      setStarted(false);
      setError(message(e));
    }
  };

  const reveal = async () => {
    setError(null);
    try {
      await revealInFinder(id);
    } catch (e) {
      setError(message(e));
    }
  };

  let action: ReactNode = null;
  if (denied) action = null;
  else if (!cleanable)
    action = (
      <button type="button" aria-label={`Lihat ${name}`} className={`${PILL} bg-seg-track`} onClick={() => void reveal()}>
        Lihat
      </button>
    );
  else if (busy)
    action = (
      <button type="button" aria-disabled="true" aria-label={`Membersihkan ${name}`} className="text-[11.5px] text-muted">
        Membersihkan…
      </button>
    );
  else if (refused !== null)
    action = (
      <button type="button" disabled aria-label={`Bersihkan ${name}`} aria-describedby={noteId} className={`${PILL} cursor-not-allowed bg-seg-track text-muted opacity-60`}>
        Bersihkan
      </button>
    );
  else
    action = (
      <button
        type="button"
        aria-label={`${asking ? "Ya, bersihkan" : "Bersihkan"} ${name}`}
        aria-describedby={asking ? noteId : undefined}
        className={`${NEUTRAL} ${asking ? "" : "opacity-0 group-hover/row:opacity-100 group-focus-within/row:opacity-100"}`}
        onKeyDown={ignoreRepeat}
        onClick={() => void press()}
      >
        {asking ? "Ya, bersihkan" : "Bersihkan"}
      </button>
    );

  const indent = nested ? "pl-9" : "pl-1.5";
  return (
    <li
      onMouseLeave={() => confirm.id === id && confirm.cancel()}
      onBlur={(e) => {
        if (confirm.id === id && !e.currentTarget.contains(e.relatedTarget)) confirm.cancel();
      }}
    >
      <div className={`group/row flex items-center gap-2 rounded-lg py-1.5 pr-1.5 hover:bg-hover focus-within:bg-hover motion-safe:transition-colors motion-safe:duration-150 ${indent}`}>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-[550]" title={title}>
            {title}
          </span>
          <span className="block truncate text-[11px] text-subtle" title={sub}>
            {sub}
          </span>
        </span>
        <div className="grid shrink-0 justify-items-end text-right">
          <span className="min-h-[1.2em] text-[13px]">{amount}</span>
          <div className="grid h-4 items-center justify-items-end">
            <span className="-my-0.5">{action}</span>
          </div>
        </div>
      </div>
      {asking && (
        <div id={noteId} className={`pr-1.5 pb-1 text-[11px] text-muted ${indent}`}>
          <p className="truncate select-text" title={path}>
            {path}
          </p>
          {item.upperBound && confirm.bytes !== null && <p>Bisa membebaskan hingga {formatBytes(confirm.bytes)}</p>}
          {item.cleaner === "oldTemp" && <p>{TEMP_RULE}</p>}
        </div>
      )}
      {refused !== null && (
        <p id={noteId} className={`pr-1.5 pb-1 text-[11px] text-muted [overflow-wrap:anywhere] ${indent}`}>
          {refused}
        </p>
      )}
      {denied && <PermissionLine text={`Butuh izin untuk membaca ${deniedFolder(path)}`} className={indent} />}
      {line && <p className={`pr-1.5 pb-1 text-[11px] text-muted [overflow-wrap:anywhere] ${indent}`}>{line}</p>}
    </li>
  );
}
