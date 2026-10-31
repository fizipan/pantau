import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { GB, item } from "@/test/storage";

vi.mock("@tauri-apps/api/core", () => ({ invoke: () => Promise.resolve(null) }));

import { NO_STORAGE_CONFIRM, StorageRow } from "./StorageRow";

describe("StorageRow", () => {
  it("never renders a clean button for an item without a cleaner", () => {
    render(<ul><StorageRow item={item("x", { category: "check", cleaner: "none" })} confirm={NO_STORAGE_CONFIRM} /></ul>);
    expect(screen.queryByRole("button", { name: /Bersihkan|Yakin|Ya,/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Lihat/ })).toBeInTheDocument();
  });

  it("a long refusal or result wraps instead of being cut", () => {
    const long = `Masih ada proses di ${"/Users/example/Documents/proyek-yang-sangat-panjang/".repeat(3)}`;
    render(<ul><StorageRow item={item("next-1", { state: { refused: long } })} confirm={NO_STORAGE_CONFIRM} /></ul>);
    expect(screen.getByText(long)).toHaveClass("[overflow-wrap:anywhere]");
  });

  it("asks through the shared confirm and freezes the amount it showed", async () => {
    const ask = vi.fn();
    render(<ul><StorageRow item={item("pnpm", { title: "Cache pnpm" })} confirm={{ ...NO_STORAGE_CONFIRM, ask }} /></ul>);
    await userEvent.click(screen.getByRole("button", { name: /^Bersihkan Cache pnpm/ }));
    expect(ask).toHaveBeenCalledWith("pnpm", 1 * GB);
  });

  it("shows the amount frozen at ask time, not a newer measurement", () => {
    render(
      <ul>
        <StorageRow item={item("pnpm", { upperBound: true, bytes: 8 * GB })} confirm={{ ...NO_STORAGE_CONFIRM, id: "pnpm", bytes: 9.6 * GB }} />
      </ul>,
    );
    expect(screen.getByText("Bisa membebaskan hingga 9,6 GB")).toBeInTheDocument();
  });
});
