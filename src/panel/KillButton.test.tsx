import { createEvent, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { KillButton } from "./KillButton";

describe("KillButton", () => {
  it("ignores a held Enter, so key repeat cannot pass the confirm delay", () => {
    const onPress = vi.fn();
    render(<KillButton status="confirm" name="Vitest" onPress={onPress} onForce={() => {}} />);
    const button = screen.getByRole("button", { name: "Yakin? Vitest" });
    const held = createEvent.keyDown(button, { key: "Enter", repeat: true });
    fireEvent(button, held);
    expect(held.defaultPrevented).toBe(true);
    const first = createEvent.keyDown(button, { key: "Enter" });
    fireEvent(button, first);
    expect(first.defaultPrevented).toBe(false);
  });
});
