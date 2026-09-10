// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App, type InspectionGraph, type SessionStatus } from "./App";

const graph: InspectionGraph = {
  semanticMetadata: { state: "unavailable", tables: [] },
  deepInspections: [],
  revision: 1,
  sidecars: [],
  schema: { state: "complete", objects: [], diagnostics: [], maxDecodedBytes: "16777216", decodedBytes: "0", stoppingCell: null },
  pointerMap: { diagnostics: [], applicable: false, complete: true, largestRoot: { value: 0, evidence: { page: { pageNumber: 1 }, range: { pageOffset: 52, fileOffset: 52, length: 4 }, validationRule: "sqlite_header_largest_root" } }, incrementalVacuum: { value: 0, evidence: { page: { pageNumber: 1 }, range: { pageOffset: 64, fileOffset: 64, length: 4 }, validationRule: "sqlite_header_incremental_vacuum" } }, locations: [], layout: null, lockBytePage: null, pages: [] },
  freelist: { firstTrunk: { value: 0, evidence: { page: { pageNumber: 1 }, range: { pageOffset: 32, fileOffset: 32, length: 4 }, validationRule: "sqlite_header_first_freelist_trunk" } }, declaredCount: null, trunks: [], coverage: { reason: "complete", stoppingClaim: null, evaluatedPages: 0, remainder: 0 } },
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
  pages: [1, 2, 3].map(number => ({ number, classification: { role: "table_leaf", reconciled: true, referenced: true, claims: [] }, detail: {
    allocationRole: null, kind: "table_leaf", coverage: "complete", diagnostics: [], freeblocks: [],
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
  sessionId: "session-fixture", availableRevisions: [1],
  snapshotId: "snapshot-fixture", source: graph.snapshot.source, state: "published",
  progress: { unit: "pages", completed: 3, total: 3, verifying: false, buildingTopology: false, buildingSidecars: false, buildingSchema: false }, revision: 1, coverage: graph.coverage, diagnostic: null,
};

function respond(status: () => SessionStatus) {
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({
    ok: true,
    json: () => Promise.resolve(url.endsWith("/revisions/1") || url.endsWith("/evidence") ? graph : status()),
  })));
}

describe("page atlas", () => {
  it("shows effective budgets and partial coverage in both workspaces", async () => {
    respond(() => ({ ...published, state: "stopped", operationalBudget: {
      maxProcessedCells: 12, maxPhaseUnits: 34, maxResidentBytes: 67108864,
    }, coverage: { ...graph.coverage, reason: "cell_budget", evaluated: 1, nextPage: 2, remainder: 2 } }));
    render(<App />);
    expect(await screen.findByText("Effective inspection budgets")).toBeTruthy();
    expect(screen.getByText(/Processed cells: 12/)).toBeTruthy();
    expect(screen.getByText(/Reason: cell budget/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Schema flow" }));
    expect(screen.getByText(/Processed cells: 12/)).toBeTruthy();
  });

  it("keeps sidecar consequences visible while selecting main-file pages", async () => {
    const snapshot = structuredClone(graph);
    Object.assign(snapshot, { sidecars: ["wal", "journal", "shm"].map(kind => ({
      kind, displayName: `customer-data.sqlite-${kind}`, length: "32", state: "malformed",
      consequence: kind === "wal" ? "wal_not_applied" : kind === "journal" ? "rollback_not_applied" : "shm_non_authoritative",
      fields: [{ name: "version", value: 3007000, offset: "4", length: 4 }],
      diagnostics: [{ code: "fixture_header_truncated", offset: "0", length: "32" }],
      coverage: { scope: "header", reason: "validation_stop", evaluatedBytes: "32", remainingBytes: "0" },
      wal: null, journal: null, shm: null,
    })) });
    vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? snapshot : published) })));
    const { container } = render(<App />);
    expect(await screen.findByRole("heading", { name: "Physical main-file image" })).toBeTruthy();
    expect(screen.getByText(/WAL not applied/)).toBeTruthy();
    expect(screen.getByText(/Rollback not applied/)).toBeTruthy();
    expect(screen.getByText(/SHM is non-authoritative/)).toBeTruthy();
    fireEvent.click(container.querySelector('[data-page-number="2"]')!);
    expect(screen.getByRole("heading", { name: "Page 2" })).toBeTruthy();
    expect(screen.getByText(/WAL not applied/)).toBeTruthy();
    expect(screen.getByText("Selected evidence belongs to the physical main-file image. Sidecar changes are not applied.")).toBeTruthy();
    expect(screen.getAllByText(/fixture_header_truncated/)).toHaveLength(3);
  });

  beforeEach(() => {
    window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
    respond(() => published);
  });

  afterEach(() => { cleanup(); vi.unstubAllGlobals(); });


  it("enters the freelist from header evidence and follows trunk and leaf links", async () => {
    const allocation = structuredClone(graph);
    Object.assign(allocation, {
      freelist: {
        firstTrunk: { value: 2, evidence: { page: { pageNumber: 1 }, range: { pageOffset: 32, fileOffset: 32, length: 4 }, validationRule: "sqlite_header_first_freelist_trunk" } },
        declaredCount: { value: 2, evidence: { page: { pageNumber: 1 }, range: { pageOffset: 36, fileOffset: 36, length: 4 }, validationRule: "sqlite_header_freelist_page_count" } },
        trunks: [{ page: { pageNumber: 2 }, leafCount: { value: 1, evidence: { page: { pageNumber: 2 }, range: { pageOffset: 4, fileOffset: 4100, length: 4 }, validationRule: "sqlite_freelist_leaf_count" } }, capacity: 1018, compatibilityCapacity: 1012 }],
        coverage: { reason: "complete", stoppingClaim: null, evaluatedPages: 2, remainder: 0 },
      },
    });
    allocation.relationshipClaims = [2, 3].map((target, index) => ({
      id: `free:${target}`, kind: index === 0 ? "freelist_trunk" : "freelist_leaf",
      source: { type: "page", pageNumber: target - 1 }, target: { pageNumber: target }, state: "validated",
      evidence: { page: { pageNumber: target - 1 }, range: { pageOffset: index === 0 ? 32 : 8, fileOffset: index === 0 ? 32 : 4104, length: 4 }, validationRule: "sqlite_freelist_pointer" },
    }));
    allocation.relationships = allocation.relationshipClaims.map(claim => ({ claimId: claim.id, kind: claim.kind, source: claim.source, target: claim.target! }));
    allocation.traversals = [{ kind: "freelist", origin: { type: "page", pageNumber: 1 }, validatedPrefix: [{ pageNumber: 2 }], stop: null }];
    for (const page of allocation.pages.slice(1)) {
      page.classification.role = page.number === 2 ? "freelist_trunk" : "freelist_leaf";
      Object.assign(page.detail, { kind: null, allocationRole: page.number === 2 ? "freelist_trunk" : "freelist_leaf", cells: [], header: null });
    }
    vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? allocation : published) })));
    const { container } = render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Inspect freelist" }));
    expect(screen.getByRole("heading", { name: "Page 2" })).toBeTruthy();
    expect(screen.getByText("Declared free pages: 2 · Evaluated: 2")).toBeTruthy();
    expect(screen.getByText("Leaf count: 1")).toBeTruthy();
    expect(container.querySelector('[data-page-number="2"]')?.getAttribute("data-role")).toBe("freelist_trunk");
    fireEvent.click(screen.getByRole("button", { name: "Follow freelist leaf to page 3" }));
    expect(screen.getByRole("heading", { name: "Page 3" })).toBeTruthy();
    expect(screen.getByText("Freelist leaf contents are unused; no cells are interpreted.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Follow freelist leaf back to page 2" }));
    fireEvent.click(screen.getByRole("button", { name: "Follow freelist trunk back to page 1" }));
    expect(screen.getByRole("heading", { name: "Page 1" })).toBeTruthy();
  });

  it.each([false, true])("shows empty or damaged allocation without a fabricated entry link (damaged=%s)", async damaged => {
    const allocation = structuredClone(graph);
    allocation.freelist.firstTrunk!.value = damaged ? 99 : 0;
    allocation.freelist.coverage.reason = damaged ? "invalid_structure" : "complete";
    allocation.freelist.coverage.remainder = damaged ? null : 0;
    allocation.relationshipClaims = [];
    allocation.relationships = [];
    vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? allocation : published) })));
    render(<App />);
    await screen.findByRole("region", { name: "Freelist allocation" });
    expect(screen.queryByRole("button", { name: "Inspect freelist" })).toBeNull();
    expect(Boolean(screen.queryByText("The freelist is empty."))).toBe(!damaged);
    if (damaged) expect(screen.getByText("Allocation coverage: invalid structure · Remaining: unknown")).toBeTruthy();
  });

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
      progress: { unit: "pages", completed: 1, total: 3, verifying: false, buildingTopology: false, buildingSidecars: false, buildingSchema: false } };
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

it("explains every supported role and finding cue in the atlas legend", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  respond(() => published);
  render(<App />);
  const legend = await screen.findByRole("region", { name: "Role and finding legend" });
  for (const label of ["table leaf", "table interior", "index leaf", "index interior", "B-tree (subtype unknown)", "freelist trunk", "freelist leaf", "Freelist (subtype unknown)", "overflow", "pointer map", "lock byte", "Unknown / opaque", "conflicting", "Unreferenced", "Warning", "Error", "Partial", "Not reconciled"]) {
    expect(legend.textContent).toContain(label);
  }
  cleanup(); vi.unstubAllGlobals();
});

it("uses reconciled roles and exposes all claims instead of prioritizing local headers", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  const classified = structuredClone(graph);
  Object.assign(classified.pages[0], { classification: { role: "conflicting", reconciled: true, referenced: true, claims: [
    { role: "table_leaf", source: "local_structure", state: "validated", evidence: graph.relationshipClaims[0].evidence, relationshipId: null },
    { role: "overflow", source: "relationship", state: "conflicting", evidence: graph.relationshipClaims[1].evidence, relationshipId: "overflow:cell:2:0" },
  ] } });
  Object.assign(classified.pages[1], { classification: { role: "unknown", reconciled: true, referenced: false, claims: [] } });
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? classified : published) })));
  const { container } = render(<App />);
  await screen.findByRole("heading", { name: "Page atlas" });
  expect(container.querySelector('[data-page-number="1"]')?.getAttribute("data-role")).toBe("conflicting");
  expect(container.querySelector('[data-page-number="2"]')?.getAttribute("data-role")).toBe("unknown");
  expect(container.querySelector('[data-page-number="2"]')?.textContent).toContain("Unreferenced");
  expect(screen.getByRole("table", { name: "Page role claims" }).textContent).toContain("local structure");
  expect(screen.getByRole("table", { name: "Page role claims" }).textContent).toContain("overflow");
  cleanup(); vi.unstubAllGlobals();
});

it("navigates pointer-map targets and parents while retaining malformed entry evidence", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  const mapped = structuredClone(graph);
  Object.assign(mapped, { pointerMap: {
    applicable: true, diagnostics: [], complete: false, largestRoot: { value: 1, evidence: graph.relationshipClaims[0].evidence },
    incrementalVacuum: { value: 0, evidence: graph.relationshipClaims[0].evidence }, locations: [{ pageNumber: 2 }], layout: null, lockBytePage: null,
    pages: [{ page: { pageNumber: 2 }, complete: false, diagnostics: ["pointer_map_truncated"], entries: [
      { target: { type: "page", pageNumber: 3 }, kind: "btree_child", rawKind: 5, parent: { type: "page", pageNumber: 1 }, parentValue: 1, evidence: { page: { pageNumber: 2 }, range: { pageOffset: 0, fileOffset: 4096, length: 5 }, validationRule: "sqlite_pointer_map_entry" }, state: "validated", diagnostics: [] },
      { target: { type: "page", pageNumber: 999 }, kind: null, rawKind: 99, parent: null, parentValue: null, evidence: { page: { pageNumber: 2 }, range: { pageOffset: 5, fileOffset: 4101, length: 1 }, validationRule: "sqlite_pointer_map_entry" }, state: "invalid", diagnostics: ["pointer_map_invalid_kind"] },
    ] }],
  } });
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? mapped : published) })));
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Inspect pointer map page 2" }));
  expect(screen.getByRole("table", { name: "Pointer-map entries" }).textContent).toContain("99");
  expect(screen.getByRole("table", { name: "Pointer-map entries" }).textContent).toContain("4096");
  expect(screen.queryByRole("button", { name: "Inspect mapped page 999" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Inspect mapped page 3" }));
  expect(screen.getByRole("heading", { name: "Page 3" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Inspect pointer map page 2" }));
  fireEvent.click(screen.getByRole("button", { name: "Inspect claimed parent page 1" }));
  expect(screen.getByRole("heading", { name: "Page 1" })).toBeTruthy();
  cleanup(); vi.unstubAllGlobals();
});

it("marks finding pages across the mosaic, including claim targets away from diagnostic bytes", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  respond(() => published);
  const { container } = render(<App />);
  await screen.findByRole("heading", { name: "Page atlas" });
  for (const page of [1, 2, 3]) {
    expect(container.querySelector(`[data-page-number="${page}"]`)?.getAttribute("data-finding")).toBe("error");
  }
  cleanup(); vi.unstubAllGlobals();
});

it("follows schema roots and descendants while preserving cell selection across workspaces", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  const attributed = structuredClone(graph);
  attributed.relationships = [{ claimId: "child:2:3", kind: "btree_child", source: { type: "page", pageNumber: 2 }, target: { pageNumber: 3 } }];
  Object.assign(attributed, { schema: { state: "complete", diagnostics: [], maxDecodedBytes: "10000", decodedBytes: "100", stoppingCell: null, objects: [
    { identity: { pageNumber: 1, index: 0 }, evidence: [], objectType: "table", name: "items", tableName: "items", declaration: "CREATE TABLE items(value)", rootPage: "2", root: { pageNumber: 2 }, pages: [{ pageNumber: 2 }, { pageNumber: 3 }], state: "complete", diagnostics: [] },
    { identity: { pageNumber: 1, index: 1 }, evidence: [], objectType: "view", name: "view_only", tableName: "view_only", declaration: "CREATE VIEW view_only AS SELECT absent_function()", rootPage: "0", root: null, pages: [], state: "declaration_only", diagnostics: [] },
  ] } });
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? attributed : published) })));
  const { container } = render(<App />);
  await screen.findByRole("heading", { name: "Page atlas" });
  fireEvent.click(screen.getByRole("button", { name: "Inspect schema table items" }));
  expect(screen.getByRole("heading", { name: "Schema flow" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Root B-tree page 2" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Descendant page 3 from page 2" }));
  expect(screen.getByRole("heading", { name: "Page 3" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Select attributed cell 3:0" }));
  fireEvent.click(screen.getByRole("button", { name: "Page atlas" }));
  expect(screen.getByRole("status", { name: "Selected cell" }).textContent).toContain("cell:3:0");
  expect(container.querySelector('[data-page-number="3"]')?.getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("region", { name: "Schema attribution" }).textContent).toContain("items");
  fireEvent.click(screen.getByRole("button", { name: "Schema flow" }));
  fireEvent.click(screen.getByRole("button", { name: "Inspect schema view view_only" }));
  expect(screen.getByText("Declaration only — no directly attributable storage B-tree.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Root B-tree page/ })).toBeNull();
  expect(screen.getByText("CREATE VIEW view_only AS SELECT absent_function()")).toBeTruthy();
  cleanup(); vi.unstubAllGlobals();
});

it("shows all attribution and distinguishes rootless objects from invalid claims with duplicate names", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  const snapshot = structuredClone(graph);
  snapshot.schema = { state: "partial", diagnostics: ["schema_btree_partial"], maxDecodedBytes: "1000", decodedBytes: "100", stoppingCell: null,
    objects: [
      { identity: { pageNumber: 1, index: 0 }, evidence: [], objectType: "table", name: "same", tableName: "same", declaration: "CREATE TABLE same(x)", rootPage: "2", root: { pageNumber: 2 }, pages: [{ pageNumber: 2 }], state: "partial", diagnostics: ["schema_attribution_partial"] },
      { identity: { pageNumber: 1, index: 1 }, evidence: [], objectType: "index", name: "another", tableName: "same", declaration: null, rootPage: "2", root: { pageNumber: 2 }, pages: [{ pageNumber: 2 }], state: "complete", diagnostics: [] },
      { identity: { pageNumber: 1, index: 2 }, evidence: [], objectType: "view", name: "same", tableName: "same", declaration: "CREATE VIEW same AS SELECT 1", rootPage: "0", root: null, pages: [], state: "declaration_only", diagnostics: [] },
      { identity: { pageNumber: 1, index: 3 }, evidence: [], objectType: "table", name: "broken", tableName: "broken", declaration: "<script>fail()</script>", rootPage: "999", root: null, pages: [], state: "unavailable", diagnostics: ["schema_root_unavailable"] },
    ] };
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(url.endsWith("/revisions/1") ? snapshot : published) })));
  const { container } = render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Inspect schema table same" }));
  const attribution = screen.getByRole("region", { name: "Schema attribution" });
  expect(attribution.textContent).toContain("table same");
  expect(attribution.textContent).toContain("index another");
  expect(screen.getByText("Partial attribution: only validated traversal prefixes are shown.")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Inspect schema view same" }));
  expect(screen.getByRole("heading", { name: "Declaration only" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Inspect schema table broken" }));
  expect(screen.getByRole("heading", { name: "Attribution unavailable" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Root B-tree page/ })).toBeNull();
  expect(screen.getByText("<script>fail()</script>")).toBeTruthy();
  expect(container.querySelector("script")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Page atlas" }));
  fireEvent.click(container.querySelector('[data-page-number="3"]')!);
  expect(screen.getByText("No validated schema attribution for this page.")).toBeTruthy();
  cleanup(); vi.unstubAllGlobals();
});

it("deep-inspects only the selected cell and removes values when the selection changes", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  let finished = false;
  const target = { sessionId: "session-fixture", snapshotId: "snapshot-fixture", revision: 1, pageNumber: 2, cellIndex: 0 };
  const coverage = { phase: "payload", reason: "pending", payloadBytes: "30", reconstructedBytes: "0", remainderBytes: "30", decodedValues: 0, expectedValues: 1, evidence: [], overflowPages: [], stoppingPage: 2, stoppingPayloadOffset: "0" };
  const receipt = () => ({ id: "job-one", target, state: finished ? "completed" : "pending", budget: { maxPayloadBytes: 16777216, maxOverflowPages: 32768, maxValues: 4096 }, coverage, resultRevision: finished ? 2 : null });
  const fetcher = vi.fn().mockImplementation((url: string) => Promise.resolve({ ok: true, json: () => Promise.resolve(
    url.endsWith("/result") ? { target, revision: 2, evidence: { cell: { pageNumber: 2, index: 0 }, coverage, fields: [], columnMetadata: "unavailable" }, values: [{ field: { ordinal: 0, serialType: "57", payloadOffset: "2", byteLength: "22", source: [], columnName: null }, value: { type: "text", value: "SELECTED_PRIVATE_VALUE" } }] }
      : url.includes("deep-inspections") ? receipt()
      : url.includes("/revisions/") ? { ...graph, revision: Number(url.split("/").at(-1)), deepInspections: [] }
      : { ...published, sessionId: "session-fixture", availableRevisions: finished ? [1, 2] : [1], revision: finished ? 2 : 1 }
  ) }));
  vi.stubGlobal("fetch", fetcher);
  const { container } = render(<App />);
  await screen.findByRole("heading", { name: "Page atlas" });
  fireEvent.click(container.querySelector('[data-page-number="2"]')!);
  fireEvent.click(screen.getByRole("button", { name: "Select cell 2:0" }));
  expect(fetcher.mock.calls.some(call => String(call[0]).includes("deep-inspections"))).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Deep-inspect selected cell" }));
  expect(await screen.findByText("Deep inspection: pending")).toBeTruthy();
  const request = fetcher.mock.calls.find(call => String(call[0]).endsWith("/deep-inspections"));
  expect(JSON.parse(request![1].body).target).toEqual(target);
  expect(screen.queryByText("SELECTED_PRIVATE_VALUE")).toBeNull();
  finished = true;
  expect(await screen.findByText("SELECTED_PRIVATE_VALUE")).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Inspection revision" })).toBeTruthy();
  fireEvent.click(container.querySelector('[data-page-number="3"]')!);
  expect(screen.queryByText("SELECTED_PRIVATE_VALUE")).toBeNull();
  fetcher.mockClear();
  fireEvent.change(screen.getByRole("combobox", { name: "Inspection revision" }), { target: { value: "1" } });
  await waitFor(() => expect(fetcher.mock.calls.some(call => String(call[0]).endsWith("/revisions/1"))).toBe(true));
  cleanup(); vi.unstubAllGlobals();
});

it("cancels a pending selected-cell job without fetching values", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  let cancelled = false;
  const target = { sessionId: "session-fixture", snapshotId: "snapshot-fixture", revision: 1, pageNumber: 2, cellIndex: 0 };
  const fetcher = vi.fn().mockImplementation((url: string) => {
    if (url.endsWith("/cancel")) cancelled = true;
    return Promise.resolve({ ok: true, json: () => Promise.resolve(
      url.includes("deep-inspections") ? { id: "cancel-job", target, state: cancelled ? "cancelled" : "pending", resultRevision: null,
        coverage: { phase: "payload", reason: cancelled ? "cancelled" : "pending", payloadBytes: "30", reconstructedBytes: "10", remainderBytes: "20",
          decodedValues: 0, expectedValues: null, stoppingPage: 2, stoppingPayloadOffset: "10", evidence: [], overflowPages: [] } }
      : url.includes("/revisions/") ? graph : published
    ) });
  });
  vi.stubGlobal("fetch", fetcher);
  const { container } = render(<App />);
  await screen.findByRole("heading", { name: "Page atlas" });
  fireEvent.click(container.querySelector('[data-page-number="2"]')!);
  fireEvent.click(screen.getByRole("button", { name: "Select cell 2:0" }));
  fireEvent.click(screen.getByRole("button", { name: "Deep-inspect selected cell" }));
  fireEvent.click(await screen.findByRole("button", { name: "Cancel deep inspection" }));
  expect(await screen.findByText("Deep inspection: cancelled")).toBeTruthy();
  expect(fetcher.mock.calls.some(call => String(call[0]).endsWith("/result"))).toBe(false);
  expect(screen.queryByRole("region", { name: "Selected stored values" })).toBeNull();
  cleanup(); vi.unstubAllGlobals();
});

it("labels a refused oversized web projection without showing a partial mosaic", async () => {
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve(
    url.includes("/revisions/")
      ? { ok: false, status: 507, json: () => Promise.resolve({ state: "budget_stopped", reason: "response_byte_budget" }) }
      : { ok: true, json: () => Promise.resolve(published) }
  )));
  const { container } = render(<App />);
  expect(await screen.findByText("Web response limit reached. No partial inspection was returned.")).toBeTruthy();
  expect(container.querySelectorAll("[data-page-number]")).toHaveLength(0);
  cleanup(); vi.unstubAllGlobals();
});
