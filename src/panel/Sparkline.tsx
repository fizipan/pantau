const W = 100;
const H = 32;

/** `values` are percentages (0 to 100) on a fixed scale, so a flat disk stays flat. Empty draws nothing. */
export function Sparkline({ values, color }: { values: number[]; color: string }) {
  const y = (v: number) => H - (Math.min(100, Math.max(0, v)) / 100) * H;
  // One sample still draws a line, from edge to edge.
  const points = values.length === 1 ? [values[0], values[0]] : values;
  const step = W / Math.max(1, points.length - 1);
  const line = points.map((v, i) => `${i === 0 ? "M" : "L"}${(i * step).toFixed(2)} ${y(v).toFixed(2)}`).join(" ");
  return (
    <svg className="col-span-full mt-2 h-9 w-full overflow-visible" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" aria-hidden="true" style={{ color }}>
      {points.length > 0 && (
        <>
          <path d={`${line} L${W} ${H} L0 ${H} Z`} fill="currentColor" fillOpacity="0.18" />
          <path d={line} fill="none" stroke="currentColor" strokeWidth="1.5" vectorEffect="non-scaling-stroke" strokeLinejoin="round" />
        </>
      )}
    </svg>
  );
}
