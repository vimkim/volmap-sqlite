// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App, type InspectionGraph, type SessionStatus } from "./App";

const graph: InspectionGraph = {
  revision: 1,
  coverage: { scope: "page_inventory", evaluated: 3, total: 3, nextPage: null, reason: "complete", remainder: 0 },
  snapshot: {
    id: "snapshot-fixture",
    source: { id: "source-fixture", displayName: "customer-data.sqlite" },
    geometry: {
      pageSize: 4096,
      usableSize: 4080,
      reservedBytes: 16,
      pageCount: 3,
      textEncoding: "utf8",
    },
  },
  pages: [1, 2, 3].map(number => ({ number, detail: {
    kind: "table_leaf", coverage: "complete", diagnostics: [], freeblocks: [],
    header: { range: { pageOffset: 0, fileOffset: (number - 1) * 4096, length: 8 }, firstFreeblock: 0, cellCount: 1, contentStart: 4076, fragmentedBytes: 0, rightmostChild: null },
    regions: [{ kind: "cell_content", range: { pageOffset: 4076, fileOffset: (number - 1) * 4096 + 4076, length: 4 } }],
    cells: [{ identity: { pageNumber: number, index: 0 }, pointer: { pageOffset: 8, fileOffset: (number - 1) * 4096 + 8, length: 2 }, offset: 4076,
      range: { pageOffset: 4076, fileOffset: (number - 1) * 4096 + 4076, length: 4 }, rowid: "99", leftChild: null,
      leftChildPointer: null,
      payloadSize: 2, localPayload: { pageOffset: 4078, fileOffset: (number - 1) * 4096 + 4078, length: 2 }, overflowPage: null,
      overflowPointer: null,
      record: { state: "complete", headerSize: 2, serialTypes: ["8"] }, diagnostic: null }],
  } })),
  relationshipClaims: [
    {
      id: "btree:page:1:rightmost", kind: "btree_child", source: { type: "page", pageNumber: 1 },
      target: { pageNumber: 2 }, state: "validated",
      evidence: { page: { pageNumber: 1 }, range: { pageOffset: 120, fileOffset: 120, length: 4 }, validationRule: "sqlite_btree_interior_rightmost_child" },
    },
    {
      id: "overflow:cell:2:0", kind: "overflow", source: { type: "cell", pageNumber: 2, cellIndex: 0 },
      target: { pageNumber: 3 }, state: "validated",
      evidence: { page: { pageNumber: 2 }, range: { pageOffset: 4080, fileOffset: 8176, length: 4 }, validationRule: "sqlite_btree_first_overflow_page" },
    },
  ],
  relationships: [
    {
      claimId: "btree:page:1:rightmost", kind: "btree_child",
      source: { type: "page", pageNumber: 1 }, target: { pageNumber: 2 },
    },
    {
      claimId: "overflow:cell:2:0", kind: "overflow",
      source: { type: "cell", pageNumber: 2, cellIndex: 0 }, target: { pageNumber: 3 },
    },
  ],
  traversals: [],
  diagnostics: [{
    code: "fixture_relationship_diagnostic", severity: "error",
    evidence: [
      { page: { pageNumber: 1 }, range: { pageOffset: 120, fileOffset: 120, length: 4 }, validationRule: "fixture_rule" },
      { page: { pageNumber: 2 }, range: { pageOffset: 44, fileOffset: 4140, length: 4 }, validationRule: "fixture_rule" },
    ],
    affectedRelationships: ["btree:page:1:rightmost", "overflow:cell:2:0"], containment: "traversal_stopped",
  }],
  topologyCoverage: {
    reason: "complete",
    phase: "complete",
    evaluated: 0,
    total: 0,
    next: null,
    remainder: 0,
    nextPhase: null,
    traversalBudget: { maxBtreePages: 1000, maxOverflowPages: 1000, maxTotalPages: "18446744073709551615" },
  },
};

const published: SessionStatus = {
  snapshotId: "snapshot-fixture", source: graph.snapshot.source, state: "published",
  progress: { unit: "pages", completed: 3, total: 3, verifying: false, buildingTopology: false }, revision: 1, coverage: graph.coverage, diagnostic: null,
};

function respond(status: () => SessionStatus) {
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({
    ok: true,
    json: () => Promise.resolve(url.endsWith("/revisions/1") || url.endsWith("/evidence") ? graph : status()),
  })));
}

describe("page atlas", () => {
  beforeEach(() => {
    window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
    respond(() => published);
  });

  afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

  it("fetches the snapshot-scoped graph and renders the complete mosaic", async () => {
    const { container } = render(<App />);

    expect(await screen.findByRole("heading", { name: "Page atlas" })).toBeTruthy();
    expect(screen.getByText("customer-data.sqlite")).toBeTruthy();
    expect(screen.getByText("0 / 0 work units")).toBeTruthy();
    expect(screen.getByText("18,446,744,073,709,551,615")).toBeTruthy();
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(3);
    expect(fetch).toHaveBeenCalledWith("/api/snapshots/snapshot-fixture/revisions/1", expect.objectContaining({ cache: "no-store" }));

    fireEvent.click(container.querySelector('[data-page-number="3"]')!);
    await waitFor(() => expect(screen.getByRole("heading", { name: "Page 3" })).toBeTruthy());
    expect(screen.getByRole("region", { name: "Structural byte map" })).toBeTruthy();
    expect(screen.getByText("cell:3:0")).toBeTruthy();
    expect(screen.queryByText("cell:1:0")).toBeNull();
    expect(screen.getByText("99")).toBeTruthy();
    expect(screen.getByRole("table", { name: "Cell inventory" })).toBeTruthy();
  });

  it("follows validated relationships in both directions and jumps to diagnostic evidence", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Page atlas" });

    fireEvent.click(screen.getByRole("button", { name: "Follow btree child to page 2" }));
    expect(screen.getByRole("heading", { name: "Page 2" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Follow btree child back to page 1" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Follow overflow to page 3" }));
    expect(screen.getByRole("heading", { name: "Page 3" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Jump to page 1 byte 120" }));
    expect(screen.getByRole("heading", { name: "Page 1" })).toBeTruthy();
    expect(screen.getByText("fixture relationship diagnostic")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Jump to page 2 byte 44" }));
    expect(screen.getByRole("heading", { name: "Page 2" })).toBeTruthy();
  });

  it("shows progress without a mosaic, then adopts only the published revision", async () => {
    let status: SessionStatus = { ...published, state: "scanning", revision: null, coverage: null,
      progress: { unit: "pages", completed: 1, total: 3, verifying: false, buildingTopology: false } };
    respond(() => status);
    const { container } = render(<App />);
    expect(await screen.findByRole("heading", { name: "Scanning" })).toBeTruthy();
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(0);
    status = published;
    expect(await screen.findByRole("heading", { name: "Published revision · 1" })).toBeTruthy();
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(3);
  });

  it("labels damaged structure without hiding independent page evidence", async () => {
    const damaged: InspectionGraph = { ...graph, pages: [{ ...graph.pages[0], detail: {
      ...graph.pages[0].detail, coverage: "partial", diagnostics: ["invalid_freeblock_link"],
      cells: [{ ...graph.pages[0].detail.cells[0], range: null, rowid: null, record: null, diagnostic: "invalid_cell_pointer" }],
    } }] };
    vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({
      ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? damaged : published),
    })));
    render(<App />);
    expect(await screen.findByText("Local B-tree coverage: partial")).toBeTruthy();
    expect(screen.getByText("invalid freeblock link")).toBeTruthy();
    expect(screen.getByText("invalid cell pointer")).toBeTruthy();
    expect(screen.getByText("cell:1:0")).toBeTruthy();
    expect(screen.queryByText("99")).toBeNull();
    expect(screen.getByRole("table", { name: "Byte regions" })).toBeTruthy();
  });

  it("removes navigation when a published snapshot becomes invalidated and labels retained evidence", async () => {
    let status = published;
    respond(() => status);
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Page atlas" });
    status = { ...published, state: "invalidated", revision: null,
      coverage: { ...graph.coverage, reason: "input_changed" },
      diagnostic: { code: "input_changed", message: "Accepted input changed.", affectedInputs: ["wal"] } };
    await screen.findByRole("heading", { name: "Snapshot invalidated" });
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(0);
    expect(screen.getByRole("heading", { name: "Retained diagnostic evidence" })).toBeTruthy();
  });

  it.each([
    ["cancelled", "Inspection cancelled"],
    ["stopped", "Inspection stopped"],
    ["fatal", "Fatal database geometry"],
  ] as const)("distinguishes %s with honest incomplete coverage", async (state, label) => {
    respond(() => ({ ...published, state, revision: null,
      coverage: { ...graph.coverage, evaluated: 0, total: null, remainder: null, reason: state === "fatal" ? "fatal_geometry" : "cancelled" } }));
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: label });
    expect(screen.getByText(/Remaining: unknown/)).toBeTruthy();
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(0);
  });

  it.each(["pending", "failed"])("shows known invalidation even when evidence is %s", async outcome => {
    let status = published;
    vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => {
      if (url.endsWith("/evidence")) return outcome === "pending"
        ? new Promise(() => {})
        : Promise.reject(new Error("Evidence unavailable"));
      return Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? graph : status) });
    }));
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Page atlas" });
    status = { ...published, state: "invalidated", revision: null };
    await screen.findByRole("heading", { name: "Snapshot invalidated" });
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(0);
    expect(screen.queryByText("Page atlas unavailable")).toBeNull();
  });
});
