// The Rust side owns every rule. This file is only the shape of its data and thin wrappers around
// its commands. Field names are exactly what Rust serializes: process and system data is
// snake_case; storage data (`StorageData`, items, recent files) is camelCase.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Pressure = "normal" | "warning" | "critical";
export type ToolKind = "app" | "test" | "dev" | "agent" | "other";

export interface SystemSample {
  at_ms: number;
  cpu_pct: number;
  core_count: number;
  ram_used: number;
  ram_total: number;
  swap_used: number;
  swap_total: number;
  pressure: Pressure;
  disk_used: number;
  disk_total: number;
  disk_free: number;
}

/** Identifies one exact process: a reused PID has a different start time. */
export interface ProcId {
  pid: number;
  start_time: number;
}

export interface ProcessRow {
  pid: number;
  start_time: number;
  label: string;
  /** null when macOS would not tell us; not the same as 0. */
  footprint: number | null;
  cpu_pct: number;
  ports: number[];
  /** A hint for the UI only. Rust authorizes every kill again. */
  killable: boolean;
}

export interface ProcessGroup {
  id: string;
  kind: ToolKind;
  name: string;
  project: string | null;
  bundle: string | null;
  footprint: number;
  cpu_pct: number;
  ports: number[];
  processes: ProcessRow[];
  session_count: number;
}

export interface Leftover {
  pid: number;
  start_time: number;
  command: string;
  project: string | null;
  age_secs: number;
  kind: ToolKind;
  preselected: boolean;
  footprint: number | null;
}

export interface Snapshot {
  groups: ProcessGroup[];
  leftovers: Leftover[];
  /** The first read after the panel opened: memory is real, `cpu_pct` is not known yet and is not 0. */
  cpu_pending: boolean;
}

export type OpState =
  | "Running"
  | { Done: { freed_bytes: number | null; skipped: number } }
  | "StillAlive"
  | { Failed: string };

/**
 * The UI chooses `op_id` (a non-empty string of at most 4096 bytes). Use `kill:<groupId or pid>` for
 * a kill and `clean:<itemId>`, so a panel opened while an operation runs can match it to its row.
 * Rust keeps an operation for 60 s after it ends.
 */
export interface Operation {
  op_id: string;
  state: OpState;
  /**
   * Targets that got TERM and are still alive. `forceKill` accepts only these, using the same
   * `op_id`. Can be non-empty with `StillAlive` or with `Failed` (some failed, some still alive).
   */
  survivors: ProcId[];
}

export interface PanelData extends Snapshot {
  history: SystemSample[];
  operations: Operation[];
}

export interface KillPlan {
  needs_confirm: boolean;
  warnings: string[];
}

export type ItemState = "pending" | "ready" | "denied" | { refused: string };

export interface StorageItem {
  id: string;
  title: string;
  sub: string;
  path: string;
  category: "safe" | "check";
  cleaner: "removeDir" | "pnpmPrune" | "oldTemp" | "oldClaudeVersions" | "none";
  /** The size is a ceiling: what a cleanup frees can be smaller. */
  upperBound: boolean;
  bytes: number | null;
  measuredAtMs: number | null;
  state: ItemState;
}

export interface RecentFile {
  path: string;
  bytes: number;
  changedAtSecs: number;
  ownerGuess: string | null;
}

export interface StorageData {
  items: StorageItem[];
  recent: RecentFile[];
  recentAtMs: number | null;
  /** Folders the last scan could not read. */
  recentSkippedDirs: number;
  /** The scan root was unreadable: an empty list then means "could not look". */
  recentDenied: boolean;
  /** Project folders that could not be listed, so their `.next` and `node_modules` rows are missing. */
  deniedRoots: string[];
  /** Memory pressure is critical: automatic runs wait, manual ones still work. */
  paused: boolean;
  measuring: boolean;
  scanning: boolean;
  /** The running measure is an automatic one: a click on "Ukur ulang" turns it into the owner's, which the panel closing does not stop. */
  measuringAuto: boolean;
  /** Same for the scan of recent files. */
  scanningAuto: boolean;
}

export interface Settings {
  /** Read from the system's login items, not from the settings file. */
  launch_at_login: boolean;
  /** "Ctrl+Alt+M": modifiers joined with "+", then the key. */
  shortcut: string;
  notifications: boolean;
  /** Folders searched for project caches. Rust checks every one and sends back canonical paths. */
  scan_folders: string[];
  /** Why the saved shortcut could not be registered at startup; read-only. */
  shortcut_error?: string | null;
}

export const getSnapshot = () => invoke<PanelData>("get_snapshot");
export const planKill = (targets: ProcId[]) => invoke<KillPlan>("plan_kill", { targets });
/** Rejects with a reason when Rust refuses; `confirmed` answers `planKill`'s `needs_confirm`. */
export const startKill = (targets: ProcId[], opId: string, confirmed: boolean) =>
  invoke<void>("start_kill", { targets, opId, confirmed });
/** Only for `survivors` of the earlier kill with the same `opId`; Rust refuses anything else. */
export const forceKill = (targets: ProcId[], opId: string) => invoke<void>("force_kill", { targets, opId });

export const getStorage = () => invoke<StorageData>("get_storage");
export const refreshStorage = () => invoke<void>("refresh_storage");
export const scanRecentNow = () => invoke<void>("scan_recent_now");
/** `id` is a storage item id; Rust looks the item up, no path is sent. */
export const cleanItem = (id: string, opId: string) => invoke<void>("clean_item", { id, opId });
/** `id` is a storage item id or `recent:<recentAtMs>:<index>` into `StorageData.recent`. */
export const revealInFinder = (id: string) => invoke<void>("reveal_in_finder", { id });

export const openActivityMonitor = () => invoke<void>("open_activity_monitor");
export const openPrivacySettings = () => invoke<void>("open_privacy_settings");
export const openSettings = () => invoke<void>("open_settings");
export const panelReady = () => invoke<void>("panel_ready");
export const closePanel = () => invoke<void>("close_panel");
/** A PNG data URL, or null when the app has no readable icon. */
export const appIcon = (bundle: string) => invoke<string | null>("app_icon", { bundle });

export const getSettings = () => invoke<Settings>("get_settings");
/** Rejects with an Indonesian reason when Rust refuses; nothing is changed then, except that a login item failure leaves the rest saved. Resolves with what was saved. */
export const saveSettings = (settings: Settings) => invoke<Settings>("save_settings", { settings });
/** Opens the system folder picker; null when cancelled. The only way to add a folder. */
export const pickScanFolder = () => invoke<string | null>("pick_scan_folder");
export const closeSettings = () => invoke<void>("close_settings");

/** Subscribes to the sampler's `snapshot` event (the same `PanelData` as `getSnapshot`, operations included, so no polling); call the returned function to stop. */
export function onSnapshot(callback: (data: PanelData) => void): Promise<UnlistenFn> {
  return listen<PanelData>("snapshot", (event) => callback(event.payload));
}
