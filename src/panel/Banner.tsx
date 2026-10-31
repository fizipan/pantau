import type { Pressure } from "@/lib/api";
import { formatBytes } from "@/lib/format";

const TITLE: Record<Exclude<Pressure, "normal">, string> = {
  warning: "Tekanan memori meningkat",
  critical: "Tekanan memori tinggi",
};

/**
 * The banner is absent at normal pressure and takes no space. The announcer is always there, empty,
 * because a live region that arrives with its text is often not read; the swap figure changes every
 * sample, so it stays out of it.
 */
export function Banner({ pressure, swapUsed }: { pressure: Pressure; swapUsed: number }) {
  return (
    <>
      <div role="status" className="sr-only">
        {pressure === "normal" ? "" : TITLE[pressure]}
      </div>
      {pressure !== "normal" && (
        <div className={`flex items-center gap-2 px-3.5 py-3 text-banner-ink ${pressure === "critical" ? "bg-banner-critical" : "bg-banner"}`}>
          <span className="size-2 rounded-full bg-banner-dot" aria-hidden="true" />
          <span className="font-semibold" aria-hidden="true">
            {TITLE[pressure]}
          </span>
          <span className="ml-auto text-[12.5px]">Swap {formatBytes(swapUsed)}</span>
        </div>
      )}
    </>
  );
}
