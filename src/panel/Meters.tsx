import type { SystemSample } from "@/lib/api";
import { formatBytes } from "@/lib/format";
import { Sparkline } from "./Sparkline";

const GB = 1024 ** 3;
const gb = (bytes: number) => String(Math.round((bytes / GB) * 10) / 10).replace(".", ",");
const pct = (used: number, total: number) => (total > 0 ? Math.round((used / total) * 100) : null);

interface MeterProps {
  label: string;
  value: number | null;
  sub: string;
  values: number[];
  color: string;
}

function Meter({ label, value, sub, values, color }: MeterProps) {
  return (
    <div className="grid min-w-0 grid-cols-[1fr_auto] items-baseline gap-y-px">
      <span className="text-[12.5px] text-muted">{label}</span>
      <span className="text-xl leading-[1.2] font-semibold tracking-[-0.01em]">{value === null ? "–" : `${value}%`}</span>
      <span className="col-span-full min-h-[15px] text-[11px] whitespace-nowrap text-subtle">{sub}</span>
      <Sparkline values={values} color={color} />
    </div>
  );
}

export function Meters({
  history,
  diskUsed,
  diskTotal,
  diskFree,
}: {
  history: SystemSample[];
  diskUsed: number;
  diskTotal: number;
  diskFree: number;
}) {
  const last = history[history.length - 1];
  const ramPct = (s: SystemSample) => (s.ram_used / s.ram_total) * 100;
  const diskPct = pct(diskUsed, diskTotal);
  return (
    <div className="grid grid-cols-3 gap-4 border-b border-hairline px-3.5 pt-4 pb-3.5">
      <Meter
        label="CPU"
        value={last ? Math.round(last.cpu_pct) : null}
        sub={last ? `${last.core_count} core` : ""}
        values={history.map((s) => s.cpu_pct)}
        color="var(--cpu)"
      />
      <Meter
        label="RAM"
        value={last ? pct(last.ram_used, last.ram_total) : null}
        sub={last ? `${gb(last.ram_used)}/${gb(last.ram_total)} GB` : ""}
        values={history.filter((s) => s.ram_total > 0).map(ramPct)}
        color="var(--ram)"
      />
      <Meter
        label="Disk"
        value={diskPct}
        sub={diskTotal > 0 ? `${formatBytes(diskFree)} sisa` : ""}
        values={diskPct === null ? [] : history.map((s) => (s.disk_total > 0 ? (s.disk_used / s.disk_total) * 100 : diskPct))}
        color="var(--disk)"
      />
    </div>
  );
}
