import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import {
  closePanel,
  getSnapshot,
  onSnapshot,
  openActivityMonitor,
  openSettings,
  panelReady,
  type PanelData,
} from "@/lib/api";
import { syncColorScheme } from "@/lib/theme";
import { Banner } from "./Banner";
import { Meters } from "./Meters";
import { ProcessTab } from "./ProcessTab";
import { StorageTab } from "./StorageTab";

const TABS = [
  { id: "proses", label: "Proses", render: (data: PanelData | null) => data && <ProcessTab data={data} /> },
  { id: "penyimpanan", label: "Penyimpanan", render: (data: PanelData | null) => <StorageTab operations={data?.operations ?? []} /> },
] as const;

export function Panel() {
  const [data, setData] = useState<PanelData | null>(null);
  const [tab, setTab] = useState(0);
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const shown = useRef(false);
  // Rust shows the window on this call, so it waits for data: the first frame is then complete.
  const show = () => {
    if (shown.current) return;
    shown.current = true;
    void panelReady();
  };

  useEffect(() => {
    const stopTheme = syncColorScheme();
    let live = false;
    let stopped = false;
    let unlisten: (() => void) | undefined;
    void onSnapshot((next) => {
      live = true;
      setData(next);
    }).then((fn) => (stopped ? fn() : (unlisten = fn)));
    // The event fills in if this fails, and a stale answer must not overwrite a newer event.
    getSnapshot()
      .then((first) => live || setData(first))
      .catch(show);
    // Never stay hidden: show after 300 ms even when no data has arrived.
    const fallback = setTimeout(show, 300);
    return () => {
      clearTimeout(fallback);
      stopped = true;
      stopTheme();
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (data) show();
  }, [data]);

  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      // The active tab gets Escape first (it clears a search or a pending Yakin?) and marks it handled.
      if (e.key === "Escape" && !e.defaultPrevented) void closePanel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const select = (index: number) => {
    const next = (index + TABS.length) % TABS.length;
    setTab(next);
    tabRefs.current[next]?.focus();
  };
  const onTabKey = (e: KeyboardEvent) => {
    const move = { ArrowRight: tab + 1, ArrowLeft: tab - 1, Home: 0, End: TABS.length - 1 }[e.key];
    if (move === undefined) return;
    e.preventDefault();
    select(move);
  };

  const last = data?.history[data.history.length - 1];

  return (
    <div className="flex h-full max-h-[640px] w-[360px] flex-col overflow-hidden rounded-2xl bg-tint">
      <h1 className="sr-only">Pantau</h1>
      <Banner pressure={last?.pressure ?? "normal"} swapUsed={last?.swap_used ?? 0} />
      <Meters
        history={data?.history ?? []}
        diskUsed={last?.disk_used ?? 0}
        diskTotal={last?.disk_total ?? 0}
        diskFree={last?.disk_free ?? 0}
      />
      <div className="mx-3.5 mt-3 flex gap-0.5 rounded-lg bg-seg-track p-0.5" role="tablist" aria-label="Tampilan" onKeyDown={onTabKey}>
        {TABS.map((t, i) => (
          <button
            key={t.id}
            ref={(el) => {
              tabRefs.current[i] = el;
            }}
            id={`tab-${t.id}`}
            role="tab"
            type="button"
            aria-selected={i === tab}
            aria-controls="tabpanel"
            tabIndex={i === tab ? 0 : -1}
            className="flex-1 rounded-md py-[5px] text-[12.5px] font-medium text-muted hover:text-ink focus-visible:outline-offset-0 aria-selected:bg-seg-on aria-selected:text-ink aria-selected:shadow-seg motion-safe:transition-colors motion-safe:duration-120"
            onClick={() => setTab(i)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-2.5" id="tabpanel" tabIndex={0} role="tabpanel" aria-labelledby={`tab-${TABS[tab].id}`}>
        {TABS[tab].render(data)}
      </div>
      <footer className="flex justify-between border-t border-hairline px-3.5 py-2.5 text-[12.5px] text-muted">
        <button type="button" className="rounded-sm hover:text-ink motion-safe:transition-colors motion-safe:duration-120" onClick={() => void openActivityMonitor()}>
          Buka Activity Monitor
        </button>
        <button type="button" className="rounded-sm hover:text-ink motion-safe:transition-colors motion-safe:duration-120" onClick={() => void openSettings()}>
          Pengaturan
        </button>
      </footer>
    </div>
  );
}
