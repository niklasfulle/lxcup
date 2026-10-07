import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ArrowIcon } from "./ArrowIcon";

describe("ArrowIcon", () => {
  it("renders the default right-pointing arrow with its default size", () => {
    const { container } = render(<ArrowIcon />);
    const icon = container.querySelector("svg");

    expect(icon).toHaveClass("h-5", "w-5");
    expect(icon?.querySelector("path")).toHaveAttribute("d", "M3.5 10h12m-5-5 5 5-5 5");
  });

  it("supports a left-pointing arrow and custom sizing classes", () => {
    const { container } = render(<ArrowIcon direction="left" className="h-4 w-4" />);
    const icon = container.querySelector("svg");

    expect(icon).toHaveClass("h-4", "w-4", "shrink-0");
    expect(icon?.querySelector("path")).toHaveAttribute("d", "M16.5 10h-12m5 5-5-5 5-5");
  });
});
