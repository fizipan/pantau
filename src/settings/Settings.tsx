import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Plus, X } from "lucide-react";
import { closeSettings, getSettings, pickScanFolder, saveSettings, type Settings as SettingsData } from "@/lib/api";
import { syncColorScheme } from "@/lib/theme";
import { formatShortcut, readRecording, spokenShortcut } from "./shortcut";

/** What the form edits. `shortcut_error` is read-only and never sent back. */
type Form = Pick<SettingsData, "launch_at_login" | "shortcut" | "notifications" | "scan_folders">;
type Field = keyof Form;
type Problem = { shortcut?: string; folders?: string; other?: string };

const FIELDS: Field[] = ["launch_at_login", "shortcut", "notifications", "scan_folders"];
const toForm = (s: SettingsData): Form => ({
  launch_at_login: s.launch_at_login,
  shortcut: s.shortcut,
  notifications: s.notifications,
  scan_folders: s.scan_folders,
});
const same = <K extends Field>(a: Form[K], b: Form[K]) => JSON.stringify(a) === JSON.stringify(b);
const formsEqual = (a: Form, b: Form) => FIELDS.every((f) => same(a[f], b[f]));

const message = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/** Rust refuses with one sentence; its start says which control it is about. */
function classify(text: string): Problem {
  if (text.startsWith("Pintasan")) return { shortcut: text };
  if (text.startsWith("/")) return { folders: text };
  return { other: text };
}

const home = (path: string) => path.replace(/^\/Users\/[^/]+/, "~");

export function Settings() {
  const [form, setForm] = useState<Form | null>(null);
  const [problem, setProblem] = useState<Problem>({});
  const [loadError, setLoadError] = useState<string | null>(null);
  // `saved` is the last thing Rust confirmed; `draft` is what the window shows.
  const saved = useRef<Form | null>(null);
  const draft = useRef<Form | null>(null);
  // One save at a time, in the order the edits were made.
  const queue = useRef<Promise<void>>(Promise.resolve());
  const picking = useRef(false);
  // The newest save failed and its reason is on screen. A close is held back once, so it is read.
  const lastSaveFailed = useRef(false);

  useEffect(syncColorScheme, []);

  const load = () => {
    setLoadError(null);
    getSettings().then(
      (s) => {
        saved.current = draft.current = toForm(s);
        setForm(draft.current);
        if (s.shortcut_error) setProblem({ shortcut: s.shortcut_error });
      },
      (e) => setLoadError(message(e)),
    );
  };
  useEffect(load, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      if (e.key === "Escape" || (e.metaKey && e.key.toLowerCase() === "w")) {
        e.preventDefault();
        // The window is destroyed on close, so a save still running must finish first. If the last
        // save failed the window stays open this once: the reason is on screen, the edit not lost silently.
        void queue.current.then(() => {
          if (lastSaveFailed.current) lastSaveFailed.current = false;
          else closeSettings();
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  async function flush() {
    const sending = draft.current;
    if (!sending || !saved.current || formsEqual(sending, saved.current)) return;
    try {
      const answer = await saveSettings(sending);
      const result = toForm(answer);
      saved.current = result;
      lastSaveFailed.current = false;
      // Rust sends folders back as canonical paths; take them unless the owner has edited since.
      if (draft.current && formsEqual(draft.current, sending)) {
        draft.current = result;
        setForm(result);
      }
      // Older refusals stay until the owner edits that control; only Rust's word on the shortcut replaces them.
      setProblem((p) => ({ ...p, shortcut: answer.shortcut_error ?? undefined }));
    } catch (e) {
      lastSaveFailed.current = true;
      // Re-read what is really in effect (a login item can fail after the rest was saved), keep
      // the edits made while this save ran, and undo only what this save was about.
      const truth = toForm(await getSettings().catch(() => saved.current!));
      saved.current = truth;
      const now = draft.current ?? sending;
      const next = { ...truth };
      for (const f of FIELDS) if (!same(now[f], sending[f])) (next as Record<Field, unknown>)[f] = now[f];
      draft.current = next;
      setForm(next);
      setProblem(classify(message(e)));
    }
  }

  function edit(patch: Partial<Form>) {
    if (!draft.current) return;
    draft.current = { ...draft.current, ...patch };
    // Nothing is left to lose once the form matches what is saved, so an old failure holds no close.
    if (saved.current && formsEqual(draft.current, saved.current)) lastSaveFailed.current = false;
    setForm(draft.current);
    setProblem((p) => ({
      ...p,
      ...("shortcut" in patch && { shortcut: undefined }),
      ...("scan_folders" in patch && { folders: undefined }),
      ...("launch_at_login" in patch && { other: undefined }),
    }));
    queue.current = queue.current.then(flush);
  }

  if (loadError) {
    return (
      <Page>
        <p role="alert" className="text-[13px] text-banner-ink">
          Pengaturan tidak bisa dibaca: {loadError}
        </p>
        <button type="button" onClick={load} className="mt-3 rounded-md bg-hover px-3 py-1.5 text-[13px]">
          Coba lagi
        </button>
      </Page>
    );
  }
  if (!form) return <Page />;

  const addFolder = async () => {
    if (picking.current) return;
    picking.current = true;
    let picked: string | null = null;
    try {
      picked = await pickScanFolder();
    } catch (e) {
      setProblem({ folders: message(e) });
    } finally {
      picking.current = false;
    }
    if (picked) edit({ scan_folders: [...(draft.current?.scan_folders ?? []), picked] });
  };

  return (
    <Page>
      <h1 className="sr-only">Pengaturan Pantau</h1>
      <Group>
        <ToggleRow label="Mulai saat login" note="Pantau muncul di menu bar setiap kali Mac menyala." on={form.launch_at_login} onChange={(v) => edit({ launch_at_login: v })} />
        <ShortcutRow value={form.shortcut} onChange={(v) => edit({ shortcut: v })} />
        <ToggleRow label="Notifikasi" note="Saat memori tertekan lebih dari 30 detik, atau ruang disk di bawah 20 GB." on={form.notifications} onChange={(v) => edit({ notifications: v })} />
      </Group>
      <Problems texts={[problem.shortcut, problem.other]} />

      <section className="mt-6">
        <h2 id="folders-title" className="px-3.5 pb-1.5 text-[13px] font-[600]">
          Folder project
        </h2>
        <Group>
          {form.scan_folders.length === 0 ? (
            <p className="px-3.5 py-3 text-[12.5px] text-muted">Belum ada folder. Pantau tidak mencari cache project.</p>
          ) : (
            <ul aria-labelledby="folders-title" className="divide-y divide-hairline">
              {form.scan_folders.map((folder) => (
                <li key={folder} className="flex min-h-10 items-center justify-between gap-3 py-1 pr-2 pl-3.5">
                  <span title={folder} className="min-w-0 truncate text-[13px]">
                    {home(folder)}
                  </span>
                  <button
                    type="button"
                    aria-label={`Hapus ${home(folder)}`}
                    onClick={() => edit({ scan_folders: form.scan_folders.filter((f) => f !== folder) })}
                    className="grid size-7 shrink-0 place-items-center rounded-md text-muted hover:bg-hover"
                  >
                    <X size={14} strokeWidth={2} aria-hidden />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <button
            type="button"
            onClick={addFolder}
            className="flex min-h-10 w-full items-center gap-2 rounded-b-[10px] px-3.5 text-left text-[13px] hover:bg-hover focus-visible:outline-offset-[-2px]"
          >
            <Plus size={14} strokeWidth={2} aria-hidden />
            Tambah folder…
          </button>
        </Group>
        <Problems texts={[problem.folders]} />
        <p className="px-3.5 pt-2 text-[11px] text-subtle">
          Cache .next dicari di folder ini dan hanya dihapus lewat Bersihkan. Folder Home, Library, dan .claude tidak bisa dipilih.
        </p>
      </section>
    </Page>
  );
}

function Page({ children }: { children?: ReactNode }) {
  return (
    <div className="h-full overflow-y-auto bg-window px-5 py-5">
      <main className="mx-auto max-w-[480px]">{children}</main>
    </div>
  );
}

function Group({ children }: { children: ReactNode }) {
  return <div className="divide-y divide-hairline rounded-[10px] border border-hairline bg-group">{children}</div>;
}

function Problems({ texts }: { texts: (string | undefined)[] }) {
  return texts.filter(Boolean).map((t) => (
    <p key={t} role="alert" className="px-3.5 pt-2 text-[11px] text-banner-ink">
      {t}
    </p>
  ));
}

function ToggleRow({ label, note, on, onChange }: { label: string; note: string; on: boolean; onChange: (on: boolean) => void }) {
  const id = useId();
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-labelledby={`${id}-label`}
      aria-describedby={`${id}-note`}
      onClick={() => onChange(!on)}
      className="flex min-h-11 w-full items-center justify-between gap-4 px-3.5 py-2 text-left first:rounded-t-[10px] last:rounded-b-[10px] focus-visible:outline-offset-[-2px]"
    >
      <span>
        <span id={`${id}-label`} className="block text-[13px]">
          {label}
        </span>
        <span id={`${id}-note`} className="block text-[11px] text-subtle">
          {note}
        </span>
      </span>
      <span aria-hidden className={`relative h-[22px] w-[38px] shrink-0 rounded-full transition-colors duration-150 motion-reduce:transition-none ${on ? "bg-cpu" : "bg-switch-off"}`}>
        <span className={`absolute top-0.5 left-0.5 size-[18px] rounded-full bg-white shadow-[0_1px_2px_rgba(0,0,0,0.3)] transition-transform duration-150 motion-reduce:transition-none ${on ? "translate-x-4" : ""}`} />
      </span>
    </button>
  );
}

/** Click, then press the new combination. The button shows what is held; Escape cancels. */
function ShortcutRow({ value, onChange }: { value: string; onChange: (shortcut: string) => void }) {
  const [recording, setRecording] = useState(false);
  const [held, setHeld] = useState("");
  const [hint, setHint] = useState<string | null>(null);
  const noteId = useId();
  const shown = recording ? held || "Tekan pintasan…" : formatShortcut(value);

  const stop = () => {
    setRecording(false);
    setHeld("");
    setHint(null);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!recording) return;
    // Tab is how the owner leaves the button; let focus move.
    if (e.key === "Tab") return stop();
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") return stop();
    const r = readRecording(e.nativeEvent);
    if (r.kind === "pending") return setHeld(r.text);
    if (r.kind === "invalid") return setHint(r.reason);
    stop();
    if (r.shortcut !== value) onChange(r.shortcut);
  };
  const onKeyUp = (e: React.KeyboardEvent) => {
    if (!recording) return;
    e.preventDefault();
    const r = readRecording(e.nativeEvent);
    if (r.kind === "pending") setHeld(r.text);
  };

  return (
    <div className="px-3.5 py-2">
      <div className="flex min-h-7 items-center justify-between gap-4">
        <span>
          <span className="block text-[13px]">
            Pintasan keyboard
          </span>
          <span className="block text-[11px] text-subtle">Buka dan tutup panel dari mana saja.</span>
        </span>
        <button
          type="button"
          aria-label={`${shown}, Pintasan keyboard`}
          aria-describedby={noteId}
          onClick={() => !recording && setRecording(true)}
          onKeyDown={onKeyDown}
          onKeyUp={onKeyUp}
          onBlur={stop}
          className={`min-w-24 rounded-md px-3 py-1 text-center text-[13px] ${recording ? "bg-group text-subtle ring-2 ring-cpu" : "bg-seg-track"}`}
        >
          {shown}
        </button>
      </div>
      <span id={noteId} className="sr-only">{`Pintasan sekarang ${spokenShortcut(value)}. Tekan untuk merekam pintasan baru.`}</span>
      <div role="status" className="sr-only">
        {recording ? "Merekam. Tekan kombinasi baru, atau Escape untuk batal." : ""}
      </div>
      {hint && (
        <p role="alert" className="pt-1.5 text-[11px] text-banner-ink">
          {hint}
        </p>
      )}
    </div>
  );
}
