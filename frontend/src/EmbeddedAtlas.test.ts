// @vitest-environment jsdom

import { screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";

it("boots the embedded production asset and renders its fetched page mosaic", async () => {
  document.body.innerHTML = '<div id="root"></div>';
  window.__VOLMAP_BOOTSTRAP__ = { snapshotId: "embedded-snapshot" };
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({
    ok: true,
    json: () => Promise.resolve({
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
      pages: [{ number: 1 }, { number: 2 }],
    }),
  }));

  // The production bundle is the exact asset embedded into the Rust executable.
  // @ts-expect-error Generated JavaScript intentionally has no declaration file.
  await import("../dist/assets/app.js");

  await waitFor(() => expect(screen.getByRole("heading", { name: "Page atlas" })).toBeTruthy());
  expect(document.querySelectorAll("[data-page-number]")).toHaveLength(2);
  expect(fetch).toHaveBeenCalledWith("/api/snapshots/embedded-snapshot", { cache: "no-store" });
});
