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
  pages: [{ number: 1 }, { number: 2 }, { number: 3 }],
};

const published: SessionStatus = {
  snapshotId: "snapshot-fixture", source: graph.snapshot.source, state: "published",
  progress: { unit: "pages", completed: 3, total: 3 }, revision: 1, coverage: graph.coverage, diagnostic: null,
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
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(3);
    expect(fetch).toHaveBeenCalledWith("/api/snapshots/snapshot-fixture/revisions/1", expect.objectContaining({ cache: "no-store" }));

    fireEvent.click(container.querySelector('[data-page-number="3"]')!);
    await waitFor(() => expect(screen.getByRole("heading", { name: "Page 3" })).toBeTruthy());
  });

  it("shows progress without a mosaic, then adopts only the published revision", async () => {
    let status: SessionStatus = { ...published, state: "scanning", revision: null, coverage: null,
      progress: { unit: "pages", completed: 1, total: 3 } };
    respond(() => status);
    const { container } = render(<App />);
    expect(await screen.findByRole("heading", { name: "Scanning" })).toBeTruthy();
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(0);
    status = published;
    expect(await screen.findByRole("heading", { name: "Published revision · 1" })).toBeTruthy();
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(3);
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
});
