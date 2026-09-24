import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DataTable } from "./DataTable";

type Row = { name: string; version: string };

const columns = [
  { accessorKey: "name", header: "Name" },
  { accessorKey: "version", header: "Version" },
];

describe("DataTable", () => {
  it("shows an empty message for empty data and filters rows", () => {
    const { rerender } = render(<DataTable<Row> columns={columns} data={[]} emptyMessage="No rows" />);

    expect(screen.getByText("No rows")).toBeInTheDocument();
    rerender(<DataTable<Row> columns={columns} data={[{ name: "api", version: "1" }, { name: "worker", version: "2" }]} emptyMessage="No matches" />);

    fireEvent.change(screen.getByRole("textbox", { name: "Tabelle filtern" }), { target: { value: "api" } });
    expect(screen.getByText("api")).toBeInTheDocument();
    expect(screen.queryByText("worker")).not.toBeInTheDocument();

    fireEvent.change(screen.getByRole("textbox", { name: "Tabelle filtern" }), { target: { value: "missing" } });
    expect(screen.getByText("No matches")).toBeInTheDocument();
  });

  it("cycles sorting state and exposes accessible sort directions", () => {
    render(<DataTable<Row> columns={columns} data={[{ name: "worker", version: "2" }, { name: "api", version: "1" }]} emptyMessage="No rows" />);
    const sort = screen.getByRole("button", { name: "Name sortieren" });

    expect(sort.closest("th")).toHaveAttribute("aria-sort", "none");
    fireEvent.click(sort);
    expect(sort.closest("th")).toHaveAttribute("aria-sort", "ascending");
    expect(sort).toHaveTextContent("↑");
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("api");

    fireEvent.click(sort);
    expect(sort.closest("th")).toHaveAttribute("aria-sort", "descending");
    expect(sort).toHaveTextContent("↓");
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("worker");
  });
});
