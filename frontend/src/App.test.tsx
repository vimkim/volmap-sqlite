// @vitest-environment jsdom

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App, type InspectionGraph } from "./App";

const graph: InspectionGraph = {
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

describe("page atlas", () => {
  beforeEach(() => {
    window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "snapshot-fixture" };
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(graph),
    }));
  });

  afterEach(() => vi.unstubAllGlobals());

  it("fetches the snapshot-scoped graph and renders the complete mosaic", async () => {
    const { container } = render(<App />);

    expect(await screen.findByRole("heading", { name: "Page atlas" })).toBeTruthy();
    expect(screen.getByText("customer-data.sqlite")).toBeTruthy();
    expect(container.querySelectorAll("[data-page-number]")).toHaveLength(3);
    expect(fetch).toHaveBeenCalledWith("/api/snapshots/snapshot-fixture", { cache: "no-store" });

    fireEvent.click(container.querySelector('[data-page-number="3"]')!);
    await waitFor(() => expect(screen.getByRole("heading", { name: "Page 3" })).toBeTruthy());
  });
});
