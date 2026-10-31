import { useEffect, useState } from "react";
import type { Leftover, Operation } from "@/lib/api";
import { formatAge, formatBytes } from "@/lib/format";
import { KillButton, NO_CONFIRM, useKill } from "./KillButton";

export const LEFTOVER_OP = "kill:leftovers";
const key = (l: Leftover) => `${l.pid}:${l.start_time}`;

/** Orphaned test runners and dev tools. Hidden when there are none; the listed checks are the confirmation. */
export function LeftoverCard({ leftovers, operation }: { leftovers: Leftover[]; operation?: Operation }) {
  const [open, setOpen] = useState(false);
  // Only what the owner changed is stored, so a leftover that appears later starts from its default.
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});
  // What was listed when the list opened: a leftover that shows up later starts unchecked, so a
  // click never stops a process the owner has not seen checked.
  const [listed, setListed] = useState<ReadonlySet<string> | null>(null);
  const checked = (l: Leftover) => overrides[key(l)] ?? (listed && !listed.has(key(l)) ? false : l.preselected);
  const selected = leftovers.filter(checked);
  const present = leftovers.map(key).join(",");
  useEffect(() => {
    const now = new Set(present.split(","));
    setListed((l) => (l && [...l].some((k) => !now.has(k)) ? new Set([...l].filter((k) => now.has(k))) : l));
    setOverrides((o) => (Object.keys(o).every((k) => now.has(k)) ? o : Object.fromEntries(Object.entries(o).filter(([k]) => now.has(k)))));
  }, [present]);
  const kill = useKill({ opId: LEFTOVER_OP, targets: selected.map(({ pid, start_time }) => ({ pid, start_time })), operation, confirm: NO_CONFIRM });
  if (leftovers.length === 0) return null;

  const known = leftovers.flatMap((l) => (l.footprint === null ? [] : [l.footprint]));
  const total = known.length ? ` · ${formatBytes(known.reduce((a, b) => a + b, 0))}` : "";

  return (
    <section className="mx-1.5 mt-2 rounded-lg bg-hover px-3 py-2">
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate text-[13px] font-[550]">{leftovers.length} proses sisa</p>
          <p className="truncate text-[11px] text-subtle">Induknya sudah selesai{total}</p>
        </div>
        <button
          type="button"
          aria-expanded={open}
          className="rounded-md bg-seg-on px-2.5 py-1 text-[12.5px] font-medium shadow-seg motion-safe:transition-colors motion-safe:duration-150"
          onClick={() => {
            setListed(open ? null : new Set(leftovers.map(key)));
            setOpen(!open);
          }}
        >
          Bersihkan
        </button>
      </div>
      {open && (
        <>
          <ul className="mt-2 border-t border-hairline pt-1">
            {leftovers.map((l) => (
              <li key={key(l)}>
                <label className="flex items-center gap-2 rounded-md py-1">
                  <input
                    type="checkbox"
                    className="size-3.5 shrink-0 accent-cpu"
                    checked={checked(l)}
                    onChange={(e) => setOverrides({ ...overrides, [key(l)]: e.target.checked })}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[12.5px]" title={l.command}>
                      {l.command}
                    </span>
                    <span className="block truncate text-[11px] text-subtle">
                      {[l.project, formatAge(0, l.age_secs * 1000)].filter(Boolean).join(" · ")}
                    </span>
                  </span>
                  {l.footprint !== null && <span className="text-[11.5px] text-subtle">{formatBytes(l.footprint)}</span>}
                </label>
              </li>
            ))}
          </ul>
          <div className="mt-1.5 flex items-center justify-end gap-2">
            {kill.line && <p className="min-w-0 flex-1 text-[11px] text-muted">{kill.line}</p>}
            {selected.length > 0 || kill.status !== "idle" ? (
              <KillButton
                status={kill.status}
                name="proses sisa"
                idleLabel={`Hentikan ${selected.length} proses`}
                hover={false}
                onPress={() => void kill.stop()}
                onForce={() => void kill.force()}
              />
            ) : null}
          </div>
        </>
      )}
    </section>
  );
}
