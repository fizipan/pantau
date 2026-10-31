import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Banner } from "./Banner";

describe("Banner", () => {
  it("is an empty announcer at normal pressure and leaves no space", () => {
    const { container } = render(<Banner pressure="normal" swapUsed={0} />);
    const status = screen.getByRole("status");
    expect(status).toBeEmptyDOMElement();
    expect(status).toHaveClass("sr-only");
    expect(container.children).toHaveLength(1);
  });

  it("wording follows the level", () => {
    render(<Banner pressure="warning" swapUsed={2_254_857_830} />);
    expect(screen.getAllByText("Tekanan memori meningkat").length).toBeGreaterThan(0);
    expect(screen.getByText("Swap 2,1 GB")).toBeInTheDocument();
  });

  it("the first banner is announced: the region was there before it was filled", () => {
    const { rerender } = render(<Banner pressure="normal" swapUsed={0} />);
    const status = screen.getByRole("status");
    rerender(<Banner pressure="warning" swapUsed={2_254_857_830} />);
    expect(screen.getByRole("status")).toBe(status);
    expect(status).toHaveTextContent(/^Tekanan memori meningkat$/);
  });

  it("keeps the live region on the title, not on the changing swap figure", () => {
    render(<Banner pressure="warning" swapUsed={2_254_857_830} />);
    expect(screen.getByRole("status")).toHaveTextContent(/^Tekanan memori meningkat$/);
  });

  it("critical says high", () => {
    render(<Banner pressure="critical" swapUsed={2_254_857_830} />);
    expect(screen.getByRole("status")).toHaveTextContent("Tekanan memori tinggi");
  });
});
