import { useEffect, useState } from "react";
import type { InspectionGraph } from "./App";

export type TraversalHeader = Omit<InspectionGraph["traversals"][number], "validatedPrefix"> & {
  traversalOffset: number;
  prefixCount: number;
};
interface Steps { offset: number; total: number; nextOffset: number | null; items: { pageNumber: number }[] }

export function TraversalWindow({ base, header, size, onSelect }: {
  base: string; header: TraversalHeader; size: number; onSelect: (page: number) => void;
}) {
  const [offset, setOffset] = useState(0);
  const [steps, setSteps] = useState<Steps | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const abort = new AbortController();
    setSteps(null); setError(null);
    void fetch(`${base}/traversals/${header.traversalOffset}/pages/${offset}/${size}`, { cache: "no-store", signal: abort.signal })
      .then(async response => {
        if (!response.ok) throw new Error(response.status === 409 ? "Snapshot invalidated; navigation is withheld." : `Traversal window unavailable (${response.status}).`);
        return response.json() as Promise<Steps>;
      }).then(value => { if (!abort.signal.aborted) setSteps(value); })
      .catch((reason: unknown) => { if (!abort.signal.aborted) setError(reason instanceof Error ? reason.message : "Traversal window unavailable."); });
    return () => abort.abort();
  }, [base, header.traversalOffset, offset, size]);
  return <section aria-label="Traversal prefix">
    <h3>{header.kind} traversal from page {header.origin.pageNumber}</h3>
    <p>Validated prefix: {header.prefixCount.toLocaleString()} pages · Termination: {header.stop?.reason ?? "complete"}</p>
    {header.stop && <p>Stopping claim: {header.stop.claimId} · Intended target: {header.stop.intendedTarget?.pageNumber ?? "unknown"}</p>}
    {error ? <p role="alert">{error}</p> : !steps ? <p role="status">Loading traversal prefix…</p> : <>
      <p>Traversal prefix pages: {steps.items.length ? `${steps.offset + 1}–${steps.offset + steps.items.length}` : "0"} of {steps.total.toLocaleString()}</p>
      <button type="button" aria-label="Previous traversal prefix pages" disabled={steps.offset === 0} onClick={() => setOffset(Math.max(0, steps.offset - size))}>Previous</button>{" "}
      <button type="button" aria-label="Next traversal prefix pages" disabled={steps.nextOffset === null} onClick={() => steps.nextOffset !== null && setOffset(steps.nextOffset)}>Next</button>
      <nav aria-label="Traversal pages">{steps.items.map((page, index) => <button type="button" key={steps.offset + index} aria-label={`Traversal page ${page.pageNumber}`} onClick={() => onSelect(page.pageNumber)}>Page {page.pageNumber}</button>)}</nav>
    </>}
  </section>;
}
