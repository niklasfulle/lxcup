import { flexRender, getCoreRowModel, getFilteredRowModel, getSortedRowModel, useReactTable, type ColumnDef, type SortingState } from "@tanstack/react-table";
import { useState } from "react";

type DataTableProps<T> = Readonly<{ columns: ColumnDef<T, any>[]; data: T[]; emptyMessage: string }>;

export function DataTable<T>({ columns, data, emptyMessage }: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>([]);
  const [filter, setFilter] = useState("");
  const table = useReactTable({
    data,
    columns,
    state: { sorting, globalFilter: filter },
    onSortingChange: setSorting,
    onGlobalFilterChange: setFilter,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
  });
  return <div className="overflow-x-auto">
    <label className="mb-3 flex items-center gap-2 text-xs font-medium text-[var(--muted)]">Filtern <input aria-label="Tabelle filtern" value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Suchen…" /></label>
    {table.getRowModel().rows.length === 0 ? <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">{emptyMessage}</p> : <table><thead>{table.getHeaderGroups().map((group) => <tr key={group.id}>{group.headers.map((header) => {
      if (header.isPlaceholder) return <th key={header.id} />;
      const sorted = header.column.getIsSorted();
      return <th key={header.id} aria-sort={sortAriaValue(sorted)}><button type="button" className="border-0 bg-transparent p-0 font-inherit font-bold" aria-label={`${String(header.column.columnDef.header)} sortieren`} onClick={header.column.getToggleSortingHandler()}>{flexRender(header.column.columnDef.header, header.getContext())}{sortIndicator(sorted)}</button></th>;
    })}</tr>)}</thead><tbody>{table.getRowModel().rows.map((row) => <tr key={row.id}>{row.getVisibleCells().map((cell) => <td key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</td>)}</tr>)}</tbody></table>}
  </div>;
}

function sortAriaValue(sorted: false | "asc" | "desc") {
  if (sorted === "asc") return "ascending";
  if (sorted === "desc") return "descending";
  return "none";
}

function sortIndicator(sorted: false | "asc" | "desc") {
  if (sorted === "asc") return " ↑";
  if (sorted === "desc") return " ↓";
  return "";
}
