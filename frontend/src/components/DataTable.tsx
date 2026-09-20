import { flexRender, getCoreRowModel, getFilteredRowModel, getSortedRowModel, useReactTable, type ColumnDef, type SortingState } from "@tanstack/react-table";
import { useState } from "react";

export function DataTable<T>({ columns, data, emptyMessage }: { columns: ColumnDef<T, any>[]; data: T[]; emptyMessage: string }) {
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
  return <div className="table-wrap">
    <label className="table-filter">Filtern <input aria-label="Tabelle filtern" value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Suchen…" /></label>
    {table.getRowModel().rows.length === 0 ? <p className="empty-state">{emptyMessage}</p> : <table><thead>{table.getHeaderGroups().map((group) => <tr key={group.id}>{group.headers.map((header) => {
      if (header.isPlaceholder) return <th key={header.id} />;
      const sorted = header.column.getIsSorted();
      return <th key={header.id} aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : "none"}><button type="button" className="sort-button" aria-label={`${String(header.column.columnDef.header)} sortieren`} onClick={header.column.getToggleSortingHandler()}>{flexRender(header.column.columnDef.header, header.getContext())}{sorted === "asc" ? " ↑" : sorted === "desc" ? " ↓" : ""}</button></th>;
    })}</tr>)}</thead><tbody>{table.getRowModel().rows.map((row) => <tr key={row.id}>{row.getVisibleCells().map((cell) => <td key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</td>)}</tr>)}</tbody></table>}
  </div>;
}
