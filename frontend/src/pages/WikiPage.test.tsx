import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { WikiPage } from "./WikiPage";

function renderWiki(isAdmin = false) {
  return render(<MemoryRouter><WikiPage isAdmin={isAdmin} /></MemoryRouter>);
}

describe("WikiPage", () => {
  it("explains the product features and links to their application areas", () => {
    renderWiki(true);

    expect(screen.getByRole("heading", { name: "lxcup-Wiki" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Docker-Container entdecken und verwalten" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Einzelne und gebündelte Workflows" })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: /Update-Policies/ }).some((link) => link.getAttribute("href") === "/update-policies")).toBe(true);
    expect(screen.getAllByRole("link", { name: /Benutzerverwaltung/ }).some((link) => link.getAttribute("href") === "/admin/users")).toBe(true);
  });

  it("filters topics and displays an empty state when there are no matches", () => {
    renderWiki();
    const search = screen.getByRole("searchbox", { name: "Wiki durchsuchen" });

    fireEvent.change(search, { target: { value: "docker socket" } });
    expect(screen.getByRole("heading", { name: "Docker-Container entdecken und verwalten" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Paketinventar und Updates" })).not.toBeInTheDocument();

    fireEvent.change(search, { target: { value: "kein solches thema" } });
    expect(screen.getByText(/Keine Wiki-Themen/)).toBeInTheDocument();
  });
});
