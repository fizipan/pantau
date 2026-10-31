import { useState } from "react";
import { revealInFinder, scanRecentNow, type RecentFile } from "@/lib/api";
import { formatBytes } from "@/lib/format";
import { Heading, PermissionLine } from "./StorageRow";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des"];
const NOTE = "px-1.5 pb-1 text-[11px] text-muted";

/** "Hari ini", "Kemarin", or "28 Okt": calendar days in the owner's time zone. */
function formatDay(atSecs: number, nowMs: number) {
  const at = new Date(atSecs * 1000);
  const today = new Date(nowMs);
  const days = Math.round((new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime() - new Date(at.getFullYear(), at.getMonth(), at.getDate()).getTime()) / 86_400_000);
  return days <= 0 ? "Hari ini" : days === 1 ? "Kemarin" : `${at.getDate()} ${MONTHS[at.getMonth()]}`;
}

const tilde = (p: string) => p.replace(/^\/Users\/[^/]+/, "~");
const split = (path: string) => {
  const cut = path.lastIndexOf("/");
  return { name: path.slice(cut + 1), dir: tilde(path.slice(0, cut) || "/") };
};

/** Big files that changed in the last 7 days. Only the date, size and place are stated; whose they are stays a guess. */
export function RecentList({ recent, recentAtMs, skippedDirs, denied, scanning, scanningAuto, now }: {
  recent: RecentFile[];
  recentAtMs: number | null;
  skippedDirs: number;
  denied: boolean;
  scanning: boolean;
  /** The scan running is an automatic one: the button then makes it the owner's, which survives the panel closing. */
  scanningAuto: boolean;
  now: number;
}) {
  const [errors, setErrors] = useState<Record<string, string>>({});
  const reveal = async (index: number, path: string) => {
    setErrors((e) => ({ ...e, [path]: "" }));
    try {
      // The scan time pins the index to the list on screen; Rust refuses it after a rescan.
      await revealInFinder(`recent:${recentAtMs ?? 0}:${index}`);
    } catch (e) {
      setErrors((all) => ({ ...all, [path]: typeof e === "string" ? e : e instanceof Error ? e.message : String(e) }));
    }
  };
  return (
    <section aria-labelledby="recent-head" className="mt-3">
      <Heading
        id="recent-head"
        aside={
          <button type="button" disabled={scanning && !scanningAuto} className="rounded-sm text-[11px] text-muted hover:text-ink disabled:hover:text-muted motion-safe:transition-colors motion-safe:duration-150" onClick={() => void scanRecentNow()}>
            {scanning ? (scanningAuto ? "Selesaikan pindai" : "Memindai…") : "Pindai ulang"}
          </button>
        }
      >
        Baru masuk atau berubah
      </Heading>
      <ul>
        {recent.map((f, i) => {
          const { name, dir } = split(f.path);
          return (
            <li key={f.path}>
            <div className="group/row flex items-center gap-2 rounded-lg py-1.5 pr-1.5 pl-1.5 hover:bg-hover focus-within:bg-hover motion-safe:transition-colors motion-safe:duration-150">
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[13px] font-[550]" title={f.path}>
                  {name}
                </span>
                <span className="block truncate text-[11px] text-subtle" title={f.path}>{`${formatDay(f.changedAtSecs, now)} · ${dir}`}</span>
                {f.ownerGuess && <span className="block truncate text-[11px] text-subtle">Kemungkinan dari {f.ownerGuess}</span>}
              </span>
              <div className="grid shrink-0 justify-items-end text-right">
                <span className="text-[13px]">{formatBytes(f.bytes)}</span>
                <div className="grid h-4 items-center justify-items-end">
                  <button
                    type="button"
                    aria-label={`Lihat ${name}`}
                    className="-my-0.5 rounded-md bg-seg-track px-2 py-0.5 text-[11.5px] font-medium"
                    onClick={() => void reveal(i, f.path)}
                  >
                    Lihat
                  </button>
                </div>
              </div>
            </div>
            {errors[f.path] && <p className={NOTE}>{errors[f.path]}</p>}
            </li>
          );
        })}
      </ul>
      {denied && <PermissionLine text="Butuh izin untuk membaca folder yang dipindai" className="pl-1.5" />}
      {!denied && skippedDirs > 0 && <p className={NOTE}>{skippedDirs} folder tidak bisa dibaca, jadi daftar ini bisa kurang lengkap</p>}
      {!denied && recent.length === 0 && (
        <p className="px-1.5 py-1 text-[12.5px] text-muted">{recentAtMs === null ? (scanning ? "Memindai…" : "Belum dipindai") : "Tidak ada file besar baru dalam 7 hari"}</p>
      )}
    </section>
  );
}
