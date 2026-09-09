// @vitest-environment jsdom

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";

it("boots the embedded production asset and renders its fetched page mosaic", async () => {
  document.body.innerHTML = '<div id="root"></div>';
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "embedded-snapshot" };
  vi.stubGlobal("fetch", vi.fn().mockImplementation((url: string) => Promise.resolve({
    ok: true,
    json: () => Promise.resolve(url.endsWith("/revisions/1") ? {
      revision: 1,
      semanticMetadata: { state: "unavailable", tables: [] },
      deepInspections: [],
      sidecars: [],
  schema: { state: "complete", objects: [{ identity: { pageNumber: 1, index: 0 }, evidence: [], objectType: "view", name: "declared_view", tableName: "declared_view", rootPage: "0", root: null, pages: [], declaration: "CREATE VIEW declared_view AS SELECT absent_function()", state: "declaration_only", diagnostics: [] }], diagnostics: [], maxDecodedBytes: "16777216", decodedBytes: "0", stoppingCell: null },
      pointerMap: { diagnostics: [], applicable: false, complete: true, largestRoot: { value: 0 }, incrementalVacuum: { value: 0 }, locations: [], layout: null, lockBytePage: null, pages: [] },
      freelist: { firstTrunk: null, declaredCount: null, trunks: [], coverage: { reason: "not_inspected", stoppingClaim: null, evaluatedPages: 0, remainder: null } },
      coverage: { scope: "page_inventory", evaluated: 2, total: 2, nextPage: null, reason: "complete", remainder: 0 },
      snapshot: {
        id: "embedded-snapshot",
        source: { id: "embedded-source", displayName: "embedded.sqlite" },
        geometry: {
          pageSize: 4096,
          usableSize: 4096,
          reservedBytes: 0,
          pageCount: 2,
          textEncoding: "utf8",
        },
      },
      pages: [1, 2].map(number => ({ number, classification: { role: "unknown", reconciled: true, referenced: false, claims: [] }, detail: { allocationRole: null, kind: null, coverage: "unsupported", header: null, regions: [], cells: [], freeblocks: [], diagnostics: [] } })),
      relationshipClaims: [], relationships: [], traversals: [], diagnostics: [],
      topologyCoverage: {
        reason: "complete", phase: "complete", evaluated: 0, total: 0, next: null, remainder: 0, nextPhase: null,
        traversalBudget: { maxBtreePages: 1000, maxOverflowPages: 1000, maxTotalPages: "10000" },
      },
    } : {
      sessionId: "embedded-session", availableRevisions: [1],
      snapshotId: "embedded-snapshot", source: { id: "embedded-source", displayName: "embedded.sqlite" },
      state: "published", revision: 1, progress: { unit: "pages", completed: 2, total: 2 },
      coverage: { scope: "page_inventory", evaluated: 2, total: 2, nextPage: null, reason: "complete", remainder: 0 },
      diagnostic: null,
    }),
  })));

  // The production bundle is the exact asset embedded into the Rust executable.
  // @ts-expect-error Generated JavaScript intentionally has no declaration file.
  await import("../dist/assets/app.js");

  await waitFor(() => expect(screen.getByRole("heading", { name: "Page atlas" })).toBeTruthy());
  expect(document.querySelectorAll("[data-page-number]")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "Inspect schema view declared_view" }));
  expect(await screen.findByRole("heading", { name: "Schema flow" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "Declaration only" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Page atlas" }));
  await waitFor(() => expect(document.querySelectorAll("[data-page-number]")).toHaveLength(2));
  expect(fetch).toHaveBeenCalledWith("/api/snapshots/embedded-snapshot/revisions/1", expect.objectContaining({ cache: "no-store" }));
});
