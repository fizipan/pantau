import { useEffect, useId, useState, type ReactNode } from "react";
import { AppWindow, Bot, Box, ChevronRight, FlaskConical, Terminal, type LucideIcon } from "lucide-react";
import { appIcon, type Operation, type ProcId, type ToolKind } from "@/lib/api";
import { KillButton, useKill, type Confirm } from "./KillButton";

const TOOL_ICON: Record<ToolKind, LucideIcon> = { app: AppWindow, test: FlaskConical, dev: Terminal, agent: Bot, other: Box };

// An app's icon does not change while the webview lives, so each bundle is asked for once, even when
// the answer is null.
const icons = new Map<string, Promise<string | null>>();
function loadIcon(bundle: string) {
  let icon = icons.get(bundle);
  if (!icon) {
    icon = appIcon(bundle).catch(() => null);
    icons.set(bundle, icon);
  }
  return icon;
}

function RowIcon({ kind, bundle }: { kind: ToolKind; bundle: string | null }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    if (bundle) void loadIcon(bundle).then((icon) => live && setSrc(icon));
    return () => {
      live = false;
    };
  }, [bundle]);
  if (src) return <img src={src} alt="" draggable={false} className="size-7 shrink-0" />;
  const Icon = TOOL_ICON[kind];
  return (
    <span className="grid size-7 shrink-0 place-items-center">
      <Icon aria-hidden="true" strokeWidth={1.75} className="size-[18px] text-muted" />
    </span>
  );
}

export interface RowProps {
  title: string;
  subline: string;
  /** Null for a process macOS would not read: no amount and no button. */
  primary: string | null;
  secondary: string | null;
  /** Null for a nested process, which has no icon. */
  icon: { kind: ToolKind; bundle: string | null } | null;
  /** Killable processes only. */
  targets: ProcId[];
  opId: string;
  operation?: Operation;
  confirm: Confirm;
  /** Set for a group: its head becomes the button that expands it. */
  expanded?: boolean;
  onToggle?: () => void;
  nested?: boolean;
  children?: ReactNode;
}

export function ProcessRow({ title, subline, primary, secondary, icon, targets, opId, operation, confirm, expanded, onToggle, nested, children }: RowProps) {
  const kill = useKill({ opId, targets, operation, confirm });
  const warnings = confirm.id === opId ? confirm.warnings : [];
  const warnId = useId();
  const hasAction = targets.length > 0;
  const covered = kill.status !== "idle";
  // Messages start under the row's text: after the chevron and icon of a group, 36px for a nested process, flush for a search result.
  const indent = nested ? "pl-9" : icon ? "pl-[62px]" : "pl-1.5";

  const text = (
    <>
      {onToggle && (
        <ChevronRight aria-hidden="true" strokeWidth={2} className={`size-3 shrink-0 text-muted motion-safe:transition-transform motion-safe:duration-150 ${expanded ? "rotate-90" : ""}`} />
      )}
      {icon && <RowIcon {...icon} />}
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] font-[550]" title={title}>
          {title}
        </span>
        <span className="block truncate text-[11px] text-subtle" title={subline}>
          {subline}
        </span>
      </span>
    </>
  );

  return (
    <li
      data-rowitem=""
      data-opid={opId}
      onMouseLeave={() => confirm.id === opId && confirm.cancel()}
      onBlur={(e) => {
        if (confirm.id === opId && !e.currentTarget.contains(e.relatedTarget)) confirm.cancel();
      }}
    >
      <div className={`group/row flex items-center gap-2 rounded-lg py-1.5 pr-1.5 hover:bg-hover focus-within:bg-hover motion-safe:transition-colors motion-safe:duration-150 ${nested ? "pl-9" : "pl-1.5"}`}>
        {onToggle ? (
          <button type="button" data-nav="" aria-expanded={expanded} className="flex min-w-0 flex-1 items-center gap-2 rounded-md text-left" onClick={onToggle}>
            {text}
          </button>
        ) : (
          <div className="flex min-w-0 flex-1 items-center gap-2">{text}</div>
        )}
        <div className="grid shrink-0 justify-items-end text-right">
          {primary && <span className="text-[13px]">{primary}</span>}
          <div className="grid h-4 items-center justify-items-end">
            {secondary && (
              <span className={`col-start-1 row-start-1 text-[11px] text-subtle motion-safe:transition-opacity motion-safe:duration-150 ${covered ? "opacity-0" : hasAction ? "group-hover/row:opacity-0 group-focus-within/row:opacity-0" : ""}`}>
                {secondary}
              </span>
            )}
            {hasAction && (
              <span className="col-start-1 row-start-1 -my-0.5">
                <KillButton status={kill.status} name={title} nav={!onToggle} describedBy={warnings.length ? warnId : undefined} onPress={() => void kill.press()} onForce={() => void kill.force()} />
              </span>
            )}
          </div>
        </div>
      </div>
      {warnings.length > 0 && (
        <div id={warnId}>
          {warnings.map((w) => (
            <Line key={w} text={w} indent={indent} />
          ))}
        </div>
      )}
      {kill.line && <Line text={kill.line} indent={indent} />}
      {children}
    </li>
  );
}

function Line({ text, indent }: { text: string; indent: string }) {
  return <p className={`pb-1 pr-1.5 text-[11px] text-muted [overflow-wrap:anywhere] ${indent}`}>{text}</p>;
}
