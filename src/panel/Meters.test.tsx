import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SystemSample } from "@/lib/api";
import { Meters } from "./Meters";

const GB = 1024 ** 3;
const sample = (over: Partial<SystemSample> = {}): SystemSample => ({
  at_ms: 0,
  cpu_pct: 62.6,
  core_count: 10,
  ram_used: 12.6 * GB,
  ram_total: 16 * GB,
  swap_used: 0,
  swap_total: 0,
  pressure: "normal",
  disk_used: 0,
  disk_total: 0,
  disk_free: 0,
  ...over,
});

const disk = { diskUsed: 335 * GB, diskTotal: 460 * GB, diskFree: 95 * GB };

describe("Meters", () => {
  it("shows the three percentages with their sublines", () => {
    render(<Meters history={[sample({ cpu_pct: 40 }), sample()]} {...disk} />);
    expect(screen.getByText("63%")).toBeInTheDocument();
    expect(screen.getByText("79%")).toBeInTheDocument();
    expect(screen.getByText("73%")).toBeInTheDocument();
    expect(screen.getByText("10 core")).toBeInTheDocument();
    expect(screen.getByText("12,6/16 GB")).toBeInTheDocument();
    expect(screen.getByText("95,0 GB sisa")).toBeInTheDocument();
  });

  it("renders with an empty history", () => {
    render(<Meters history={[]} diskUsed={0} diskTotal={0} diskFree={0} />);
    expect(screen.getAllByText("–")).toHaveLength(3);
  });
});
