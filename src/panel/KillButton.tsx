import { useEffect, useRef, useState } from "react";
import { forceKill, planKill, startKill, type Operation, type ProcId } from "@/lib/api";

/** One pending `Yakin?` for the whole list, owned by `ProcessTab` so a snapshot or Escape can reach it. */
export interface Confirm {
  id: string | null;
  warnings: string[];
  /** The exact targets the warnings were written for; the second click kills these and no others. */
  targets: ProcId[];
  ask: (id: string, warnings: string[], targets: ProcId[]) => void;
  cancel: () => void;
}

export const NO_CONFIRM: Confirm = { id: null, warnings: [], targets: [], ask: () => {}, cancel: () => {} };

export type KillStatus = "idle" | "confirm" | "busy" | "force";

/** A second press this soon after `Yakin?` appears is a double click or key repeat, not a decision. */
const CONFIRM_DELAY_MS = 400;
const same = (a: ProcId, b: ProcId) => a.pid === b.pid && a.start_time === b.start_time;

const message = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/**
 * Kill, confirm, progress and failure for one `opId`. Rust owns every rule: `planKill` only decides
 * whether to ask first, and what the operation says (not a local guess) decides Paksa.
 */
export function useKill({ opId, targets, operation, confirm }: { opId: string; targets: ProcId[]; operation?: Operation; confirm: Confirm }) {
  // Bridges the gap between the click and the first snapshot that carries the operation.
  const [started, setStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const planning = useRef(false);
  const askedAt = useRef(0);
  useEffect(() => {
    if (operation) setStarted(false);
  }, [operation]);
  // A new operation state supersedes a local error from an earlier attempt.
  const stateKey = operation ? JSON.stringify(operation.state) : "";
  useEffect(() => setError(null), [stateKey]);

  const survivors = operation?.survivors ?? [];
  const failed = operation && typeof operation.state === "object" && "Failed" in operation.state ? operation.state.Failed : null;
  const status: KillStatus = started
    ? "busy"
    : survivors.length > 0
      ? "force"
      : operation?.state === "Running"
        ? "busy"
        : confirm.id === opId
          ? "confirm"
          : "idle";

  const run = async (go: () => Promise<void>) => {
    setError(null);
    setStarted(true);
    try {
      await go();
    } catch (e) {
      setStarted(false);
      setError(message(e));
    }
  };
  const stop = (which = targets) => {
    confirm.cancel();
    return run(() => startKill(which, opId, true));
  };
  const press = async () => {
    if (planning.current) return;
    if (confirm.id === opId) {
      if (targets.some((t) => !confirm.targets.some((c) => same(c, t)))) {
        // The group grew since the warning was written: it no longer describes what would die.
        confirm.cancel();
      } else {
        if (Date.now() - askedAt.current < CONFIRM_DELAY_MS) return;
        return stop(confirm.targets);
      }
    }
    planning.current = true;
    setError(null);
    try {
      const plan = await planKill(targets);
      if (plan.needs_confirm) {
        askedAt.current = Date.now();
        confirm.ask(opId, plan.warnings, targets);
      }
      else await run(() => startKill(targets, opId, false));
    } catch (e) {
      setError(message(e));
    } finally {
      planning.current = false;
    }
  };
  const force = () => run(() => forceKill(survivors, opId));

  return { status, line: error ?? (started ? null : failed), press, stop, force };
}

const BUTTON =
  "rounded-md bg-kill px-2 py-0.5 text-[11.5px] font-medium text-white motion-safe:transition-opacity motion-safe:duration-150";

/** `hover` makes an idle button appear with its row; the other states are always shown, since they carry news. */
export function KillButton({ status, name, idleLabel, hover = true, nav = false, describedBy, onPress, onForce }: {
  status: KillStatus;
  name: string;
  /** Replaces `Kill` for a button that is not about one row, such as the leftover card's. */
  idleLabel?: string;
  hover?: boolean;
  nav?: boolean;
  describedBy?: string;
  onPress: () => void;
  onForce: () => void;
}) {
  // Still a button, so focus stays where the owner put it instead of falling to the page.
  if (status === "busy")
    return (
      <button type="button" data-nav={nav ? "" : undefined} aria-disabled="true" aria-label={`Menghentikan ${name}`} className="text-[11.5px] text-muted">
        Menghentikan…
      </button>
    );
  const idle = status === "idle";
  const show = !idle || !hover ? "" : "opacity-0 group-hover/row:opacity-100 group-focus-within/row:opacity-100";
  const label = status === "force" ? "Paksa" : status === "confirm" ? "Yakin?" : (idleLabel ?? "Kill");
  return (
    <button
      type="button"
      data-nav={nav ? "" : undefined}
      aria-label={idle && idleLabel ? undefined : `${label} ${name}`}
      aria-describedby={status === "confirm" ? describedBy : undefined}
      className={`${BUTTON} ${show}`}
      // Holding Enter repeats keydown; a repeat must not pass the confirm delay.
      onKeyDown={(e) => e.repeat && (e.key === "Enter" || e.key === " ") && e.preventDefault()}
      onClick={status === "force" ? onForce : onPress}
    >
      {label}
    </button>
  );
}
