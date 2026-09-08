import { useEffect, useMemo, useState } from "react";
import {
  PageDetail, roleLabel, type PageEvidence, type PhysicalEvidence, type Relationship,
  type RelationshipClaim, type StructuralDiagnostic,
} from "./PageDetail";

type TextEncoding = "utf8" | "utf16_le" | "utf16_be";
type TopologyPhase = "btree_claim_collection" | "btree_claim_validation" | "btree_parent_reconciliation" | "btree_cycle_reconciliation" | "btree_relationship_normalization" | "btree_traversal" | "overflow_inspection" | "overflow_reconciliation" | "overflow_relationship_normalization" | "complete";

export interface InspectionGraph {
  revision: number;
  coverage: Coverage;
  snapshot: {
    id: string;
    source: { id: string; displayName: string };
    geometry: {
      pageSize: number;
      usableSize: number;
      reservedBytes: number;
      pageCount: number;
      textEncoding: TextEncoding;
    };
  };
  pages: Array<{ number: number; detail: PageEvidence }>;
  relationshipClaims: RelationshipClaim[];
  relationships: Relationship[];
  traversals: Array<{
    kind: "btree" | "overflow";
    origin: { type: "page" | "cell"; pageNumber: number; cellIndex?: number };
    validatedPrefix: Array<{ pageNumber: number }>;
    stop: {
      reason: "missing_target" | "out_of_range" | "coverage_stop" | "invalid_reference" | "type_mismatch" | "cycle" | "conflicting_claim" | "overlapping_extent" | "budget" | "cancelled" | "operator_stop";
      claimId: string;
      intendedTarget: { pageNumber: number } | null;
    } | null;
  }>;
  diagnostics: StructuralDiagnostic[];
  topologyCoverage: {
    reason: "complete" | "budget" | "cancelled" | "operator_stop";
    phase: TopologyPhase;
    evaluated: number;
    total: number | null;
    next: number | null;
    remainder: number | null;
    nextPhase: TopologyPhase | null;
    traversalBudget: { maxBtreePages: number; maxOverflowPages: number; maxTotalPages: string };
  };
}

interface Coverage {
  scope: "page_inventory";
  evaluated: number;
  total: number | null;
  nextPage: number | null;
  reason: "complete" | "cancelled" | "operator_stop" | "input_changed" | "fatal_geometry" | "allocation_failure";
  remainder: number | null;
}

export interface SessionStatus {
  snapshotId: string;
  source: { id: string; displayName: string };
  state: "scanning" | "published" | "cancelled" | "stopped" | "invalidated" | "fatal";
  progress: { unit: "pages"; completed: number; total: number | null; verifying: boolean; buildingTopology: boolean };
  revision: number | null;
  coverage: Coverage | null;
  diagnostic: { code: string; message: string; affectedInputs: string[] } | null;
}

declare global {
  interface Window {
    __VOLMAP_BOOTSTRAP__: { snapshotId: string };
  }
}

const encodingLabel: Record<TextEncoding, string> = {
  utf8: "UTF-8",
  utf16_le: "UTF-16 LE",
  utf16_be: "UTF-16 BE",
};

function Atlas({ graph, status }: { graph: InspectionGraph; status: SessionStatus }) {
  const { geometry, source } = graph.snapshot;
  const [selectedPage, setSelectedPage] = useState(1);
  const [evidenceLocus, setEvidenceLocus] = useState<PhysicalEvidence | null>(null);
  const projection = useMemo(() => {
    const pagesByNumber = new Map(graph.pages.map(page => [page.number, page]));
    const claimsByPage = new Map<number, RelationshipClaim[]>();
    for (const claim of graph.relationshipClaims) {
      const source = claim.source.pageNumber;
      const touched = claim.target?.pageNumber === source
        ? [source]
        : [source, claim.target?.pageNumber].filter((page): page is number => page !== undefined);
      for (const page of touched) {
        const claims = claimsByPage.get(page);
        if (claims) claims.push(claim);
        else claimsByPage.set(page, [claim]);
      }
    }
    return {
      pagesByNumber,
      claimsByPage,
      relationshipsByClaim: new Map(
        graph.relationships.map(relationship => [relationship.claimId, relationship]),
      ),
    };
  }, [graph.pages, graph.relationshipClaims, graph.relationships]);
  const active = projection.pagesByNumber.get(selectedPage);
  const selectPage = (page: number) => {
    setSelectedPage(page);
    setEvidenceLocus(null);
  };

  return (
    <main className="workspace">
      <header className="topbar">
        <div>
          <p className="eyebrow">Volmap SQLite Inspector</p>
          <h1>Page atlas</h1>
        </div>
        <div className="source-badge">
          <span className="status-dot" />
          <span>{source.displayName}</span>
        </div>
      </header>

      <InspectionNotice status={status} />

      <section className="geometry" aria-label="Snapshot geometry">
        <GeometryFact label="Pages" value={geometry.pageCount.toLocaleString()} />
        <GeometryFact label="Page size" value={`${geometry.pageSize.toLocaleString()} B`} />
        <GeometryFact label="Usable size" value={`${geometry.usableSize.toLocaleString()} B`} />
        <GeometryFact label="Reserved" value={`${geometry.reservedBytes} B`} />
        <GeometryFact label="Encoding" value={encodingLabel[geometry.textEncoding]} />
        <GeometryFact label="Topology coverage" value={graph.topologyCoverage.reason} />
        <GeometryFact label="Topology phase" value={graph.topologyCoverage.phase.replaceAll("_", " ")} />
        <GeometryFact
          label="Topology phase progress"
          value={`${graph.topologyCoverage.evaluated.toLocaleString()} / ${graph.topologyCoverage.total?.toLocaleString() ?? "unknown"} work units`}
        />
        <GeometryFact label="Topology next unit" value={graph.topologyCoverage.next?.toLocaleString() ?? "none"} />
        <GeometryFact label="Topology remainder" value={graph.topologyCoverage.remainder?.toLocaleString() ?? "unknown"} />
        <GeometryFact label="Topology next phase" value={graph.topologyCoverage.nextPhase?.replaceAll("_", " ") ?? "none"} />
        <GeometryFact label="B-tree path limit" value={graph.topologyCoverage.traversalBudget.maxBtreePages.toLocaleString()} />
        <GeometryFact label="Overflow path limit" value={graph.topologyCoverage.traversalBudget.maxOverflowPages.toLocaleString()} />
        <GeometryFact label="Aggregate traversal limit" value={BigInt(graph.topologyCoverage.traversalBudget.maxTotalPages).toLocaleString()} />
      </section>

      <div className="content-grid">
        <section className="atlas-panel">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Physical projection</p>
              <h2>{graph.coverage.reason === "complete" ? "Main-file mosaic" : "Inspected page prefix"}</h2>
            </div>
            <p>{graph.pages.length.toLocaleString()} inspected pages</p>
          </div>
          <div className="mosaic">
            {graph.pages.map((page) => (
              <button
                className={page.number === selectedPage ? "page selected" : "page"}
                data-page-number={page.number}
                data-role={page.detail.kind ?? "unknown"}
                aria-pressed={page.number === selectedPage}
                key={page.number}
                onClick={() => selectPage(page.number)}
                type="button"
              >
                <span>Page</span>
                <strong>{page.number}</strong>
                <span>{roleLabel(page.detail.kind)}</span>
                {page.detail.coverage === "partial" && <span>Partial</span>}
              </button>
            ))}
          </div>
        </section>

        <aside className="evidence-panel">
          <p className="eyebrow">Selection-linked evidence</p>
          <h2>{graph.pages.length ? `Page ${selectedPage}` : "No pages inspected"}</h2>
          {graph.pages.length > 0 && <>
          <dl>
            <div><dt>Identity</dt><dd>page:{selectedPage}</dd></div>
            <div><dt>Byte range</dt><dd>{((selectedPage - 1) * geometry.pageSize).toLocaleString()}–{(selectedPage * geometry.pageSize - 1).toLocaleString()}</dd></div>
            <div><dt>Role claim</dt><dd>{active ? roleLabel(active.detail.kind) : "Unknown"}</dd></div>
          </dl>
          </>}
          <p className="evidence-note">
            Select a page to inspect its structural map and physical cell inventory below. Global role attribution is not yet evaluated.
          </p>
        </aside>
      </div>
      {graph.diagnostics.length > 0 && <section className="relationship-diagnostics" aria-label="Relationship diagnostics">
        <div className="section-heading"><h2>Relationship diagnostics</h2><span>{graph.diagnostics.length}</span></div>
        <ul>{graph.diagnostics.map((diagnostic, index) => {
          return <li key={`${diagnostic.code}:${index}`}>
            <strong>{diagnostic.code.replaceAll("_", " ")}</strong>
            <span>{diagnostic.severity} · {diagnostic.containment.replaceAll("_", " ")}</span>
            {diagnostic.evidence.map((evidence, evidenceIndex) => <button
              key={`${evidence.page.pageNumber}:${evidence.range.pageOffset}:${evidenceIndex}`}
              type="button"
              aria-label={`Jump to page ${evidence.page.pageNumber} byte ${evidence.range.pageOffset}`}
              onClick={() => {
                setSelectedPage(evidence.page.pageNumber);
                setEvidenceLocus(evidence);
              }}
            >
              Jump to page {evidence.page.pageNumber}, byte {evidence.range.pageOffset}
            </button>)}
          </li>;
        })}</ul>
      </section>}
      {active && <PageDetail
        detail={active.detail}
        pageSize={geometry.pageSize}
        pageNumber={active.number}
        claims={projection.claimsByPage.get(active.number) ?? []}
        relationshipsByClaim={projection.relationshipsByClaim}
        onSelectPage={selectPage}
        evidenceByte={evidenceLocus?.page.pageNumber === active.number ? evidenceLocus.range.pageOffset : null}
      />}
    </main>
  );
}

const stateLabel: Record<SessionStatus["state"], string> = {
  scanning: "Scanning",
  published: "Published revision",
  cancelled: "Inspection cancelled",
  stopped: "Inspection stopped",
  invalidated: "Snapshot invalidated",
  fatal: "Fatal database geometry",
};

function InspectionNotice({ status }: { status: SessionStatus }) {
  const { coverage, progress } = status;
  return <section className="inspection-notice" aria-label="Inspection status" aria-live="polite">
    <h2>{stateLabel[status.state]}{status.revision !== null ? ` · ${status.revision}` : ""}</h2>
    {status.state === "scanning" && <p>
      Page inventory: {progress.completed} / {progress.total ?? "unknown"} pages
      {progress.verifying && ". Verifying frozen inputs before publication."}
      {progress.buildingTopology && ". Building bounded relationship topology."}
    </p>}
    {coverage && <p>
      {coverage.reason === "complete" ? "Page inventory complete" : "Partial coverage"}: {coverage.evaluated} pages evaluated.
      {" "}Remaining: {coverage.remainder ?? "unknown"}.
      {coverage.nextPage !== null && ` Stopped before page ${coverage.nextPage}.`}
      {" "}Reason: {coverage.reason.replaceAll("_", " ")}.
    </p>}
    {status.diagnostic && <p role="alert">{status.diagnostic.message}</p>}
    {status.state === "invalidated" && <p>Navigation and further inspection are disabled. Open a new frozen copy to continue.</p>}
  </section>;
}

function GeometryFact({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

export function App() {
  const [graph, setGraph] = useState<InspectionGraph | null>(null);
  const [status, setStatus] = useState<SessionStatus | null>(null);
  const [evidence, setEvidence] = useState<Omit<InspectionGraph, "revision"> | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const snapshotId = encodeURIComponent(window.__VOLMAP_BOOTSTRAP__.snapshotId);
    const base = `/api/snapshots/${snapshotId}`;
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    let cached: InspectionGraph | null = null;
    let invalidated = false;
    async function poll() {
      try {
        const response = await fetch(base, { cache: "no-store", signal: abort.signal });
        if (!response.ok) throw new Error(`Inspection request failed (${response.status})`);
        let next: SessionStatus = await response.json();
        if (next.state === "invalidated" || next.revision === null) cached = null;
        else if (cached?.revision !== next.revision) {
          const revision = await fetch(`${base}/revisions/${next.revision}`, { cache: "no-store", signal: abort.signal });
          if (revision.status === 409) {
            next = await revision.json();
            cached = null;
          } else {
            if (!revision.ok) throw new Error("Published revision unavailable");
            cached = await revision.json();
          }
        }
        if (next.state === "invalidated") {
          invalidated = true;
          if (!abort.signal.aborted) {
            setStatus(next); setGraph(null); setEvidence(null); setError(null);
            // Retained evidence must never delay or mask known invalidation.
            void fetch(`${base}/evidence`, { cache: "no-store", signal: abort.signal })
              .then(async response => {
                if (response.ok) {
                  const retained: Omit<InspectionGraph, "revision"> = await response.json();
                  if (!abort.signal.aborted) setEvidence(retained);
                }
              })
              .catch(() => { /* The invalidation notice remains authoritative. */ });
          }
          return;
        }
        if (!abort.signal.aborted) {
          setStatus(next); setGraph(cached); setError(null);
        }
      } catch (reason: unknown) {
        if (!abort.signal.aborted) {
          setGraph(null);
          setError(reason instanceof Error ? reason.message : "Inspection request failed");
        }
      } finally {
        if (!abort.signal.aborted && !invalidated) timer = setTimeout(poll, 500);
      }
    }
    void poll();
    return () => { abort.abort(); clearTimeout(timer); };
  }, []);

  if (error) return <main className="message"><h1>Page atlas unavailable</h1><p>{error}</p></main>;
  if (!status) return <main className="message"><h1>Opening inspection…</h1></main>;
  if (graph) return <Atlas graph={graph} status={status} />;
  return <main className="workspace">
    <p className="eyebrow">Volmap SQLite Inspector</p>
    <h1>{status.source.displayName}</h1>
    <InspectionNotice status={status} />
    {status.state === "scanning" && <button type="button" onClick={() => {
      const id = encodeURIComponent(status.snapshotId);
      void fetch(`/api/snapshots/${id}/cancel`, { method: "POST" }).catch(() => setError("Cancellation request failed"));
    }}>Cancel inspection</button>}
    {evidence && <section aria-label="Retained diagnostic evidence">
      <h2>Retained diagnostic evidence</h2>
      <p>{evidence.pages.length} observed pages. These observations are not a coherent revision.</p>
      <p>Observed page numbers: {evidence.pages.map(page => page.number).join(", ") || "none"}</p>
    </section>}
  </main>;
}
