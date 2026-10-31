import type { RecentFile, StorageData, StorageItem } from "@/lib/api";

export const GB = 1024 ** 3;
/** Matches `storage-data.json`; tests set the clock relative to it. */
export const MEASURED = 1_793_000_000_000;

export const item = (id: string, o: Partial<StorageItem> = {}): StorageItem => ({
  id,
  title: `Item ${id}`,
  sub: "Contoh",
  path: `/Users/example/Library/${id}`,
  category: "safe",
  cleaner: "removeDir",
  upperBound: false,
  bytes: 1 * GB,
  measuredAtMs: MEASURED,
  state: "ready",
  ...o,
});

export const recent = (name: string, o: Partial<RecentFile> = {}): RecentFile => ({
  path: `/Users/example/Downloads/${name}`,
  bytes: 3 * GB,
  changedAtSecs: MEASURED / 1000 - 3600,
  ownerGuess: null,
  ...o,
});

export const storage = (items: StorageItem[] = [], o: Partial<StorageData> = {}): StorageData => ({
  items,
  recent: [],
  recentAtMs: MEASURED,
  recentSkippedDirs: 0,
  recentDenied: false,
  deniedRoots: [],
  paused: false,
  measuring: false,
  scanning: false,
  measuringAuto: false,
  scanningAuto: false,
  ...o,
});
