import { useEffect, useMemo, useState } from "react";
import { WindowedAtlas, type WindowNavigation } from "./WindowedAtlas";
import {
  PageDetail, type PageEvidence, type PhysicalEvidence, type Relationship,
  type RelationshipClaim, type StructuralDiagnostic,
} from "./PageDetail";

import { RoleLegend, RoleClaims, roleLabels, type Classification } from "./PageRoles";
import { PointerMap, type PointerMapEvidence } from "./PointerMap";
import { Sidecars, type SidecarEvidence } from "./Sidecars";
import { Freelist, type FreelistEvidence } from "./Freelist";

import { DeepCell, type DeepEvidence, type Budget } from "./DeepCell";
import { SchemaFlow, SchemaObjects, schemaSelector, selectorKey, type EntitySelector, type SchemaEvidence } from "./SchemaFlow";

type TextEncoding = "utf8" | "utf16_le" | "utf16_be";
type TopologyPhase = "pointer_map_validation" | "pointer_map_reconciliation" | "pointer_map_inspection" | "role_reconciliation" | "allocation_reconciliation" | "freelist_inspection" | "btree_claim_collection" | "btree_claim_validation" | "btree_parent_reconciliation" | "btree_cycle_reconciliation" | "btree_relationship_normalization" | "btree_traversal" | "overflow_inspection" | "overflow_reconciliation" | "overflow_relationship_normalization" | "complete";

export interface InspectionGraph {
  semanticMetadata: {
    coverage?: { phase: string; reason: string; evaluatedBytes: string; totalBytes: string | null; remainderBytes: string | null };
    state: "available" | "unavailable";
    tables: { identity: { pageNumber: number; index: number }; columnCount: number; strict: boolean; withoutRowid: boolean }[];
  };
  deepInspections: DeepEvidence[];
  schema: SchemaEvidence;
  sidecars: SidecarEvidence[];
  revision: number;
  freelist: FreelistEvidence;
  pointerMap: PointerMapEvidence;
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
  pages: Array<{ number: number; classification: Classification; detail: PageEvidence }>;
  relationshipClaims: RelationshipClaim[];
  relationships: Relationship[];
  traversals: Array<{
    kind: "btree" | "overflow" | "freelist";
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
  reason: "complete" | "cancelled" | "operator_stop" | "input_changed" | "fatal_geometry" | "allocation_failure" | "cell_budget" | "resident_memory_budget";
  remainder: number | null;
}

export interface SessionStatus {
  storage?: { spilled: boolean; spilledIndexes: number; cacheBytes: number; spillBytes: number; storedPages: number };
  operationalBudget?: { maxProcessedCells: number; maxPhaseUnits: number; maxResidentBytes: number; maxFreelistTrunks?: number };
  deepLimits?: { maxJobs: number; maxConcurrentJobs: number; perJob: Budget };
  webLimits?: { requestBytes: number; responseBytes: number; collectionItems: number; concurrentRequests: number };
  traversalBudget?: { maxBtreePages: number; maxOverflowPages: number; maxTotalPages: string };
  schemaBudget?: { maxDecodedBytes: number };
  sidecarBudget?: { maxWalFrames: number };
  semanticBudget?: { maxCopyBytes: number; maxSchemaRecords: number; timeoutMs: number; maxOutputBytes: number } | null;
  workCoverage?: { phase: string; evaluated: number; total: number | null; next: number | null; remainder: number | null; reason: string; limit: string | null }[];
  workProgress?: { phase: string; evaluated: number; total: number | null } | null;
  sessionId: string;
  availableRevisions: number[];
  snapshotId: string;
  source: { id: string; displayName: string };
  state: "scanning" | "published" | "cancelled" | "stopped" | "invalidated" | "fatal";
  progress: { unit: "pages"; completed: number; total: number | null; verifying: boolean; buildingTopology: boolean; buildingSidecars: boolean; buildingSchema: boolean };
  revision: number | null;
  coverage: Coverage | null;
  diagnostic: { code: string; message: string; affectedInputs: string[]; opaqueRange?: { fileOffset: string; length: string } } | null;
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

export function Atlas({ graph, status, viewRevision, onRevision, navigation }: { graph: InspectionGraph; status: SessionStatus; viewRevision: number | null; onRevision: (revision: number | null) => void; navigation?: WindowNavigation }) {
  const { geometry, source } = graph.snapshot;
  const [workspace, setWorkspace] = useState<"atlas" | "schema">("atlas");
  const [localSelection, setSelection] = useState<EntitySelector>({ type: "page", pageNumber: 1 });
  const selection = navigation?.selection ?? localSelection;
  const [selectedSchema, setSelectedSchema] = useState<string | null>(null);
  const selectedPage = selection.pageNumber;
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
    const findingsByPage = new Map<number, "warning" | "error">();
    const mark = (page: number, severity: "warning" | "error") => {
      if (findingsByPage.get(page) !== "error") findingsByPage.set(page, severity);
    };
    const claimsById = new Map(graph.relationshipClaims.map(claim => [claim.id, claim]));
    for (const diagnostic of graph.diagnostics) {
      for (const evidence of diagnostic.evidence) mark(evidence.page.pageNumber, diagnostic.severity);
      for (const id of diagnostic.affectedRelationships) {
        const claim = claimsById.get(id);
        if (claim) {
          mark(claim.source.pageNumber, diagnostic.severity);
          if (claim.target) mark(claim.target.pageNumber, diagnostic.severity);
        }
      }
    }
    for (const page of graph.pages) {
      if (page.classification.role === "conflicting" || page.detail.diagnostics.length || page.detail.cells.some(cell => cell.diagnostic) || page.detail.freeblocks.some(block => block.diagnostic)) mark(page.number, "error");
    }
    return {
      findingsByPage,
      pagesByNumber,
      claimsByPage,
      relationshipsByClaim: new Map(
        graph.relationships.map(relationship => [relationship.claimId, relationship]),
      ),
    };
  }, [graph.pages, graph.relationshipClaims, graph.relationships, graph.diagnostics]);
  const active = projection.pagesByNumber.get(selectedPage);
  const freelistTraversal = graph.traversals.find(traversal => traversal.kind === "freelist");
  const firstFreelistLink = graph.relationships.find(relationship =>
    relationship.kind === "freelist_trunk" && relationship.source.pageNumber === 1);
  const activeTrunk = graph.freelist.trunks.find(trunk => trunk.page.pageNumber === selectedPage);
  const selectEntity = (selector: EntitySelector) => {
    if (selector.type === "schema") {
      const object = graph.schema.objects.find(object => selectorKey(schemaSelector(object)) === selectorKey(selector))
        ?? navigation?.attribution.find(object => selectorKey(schemaSelector(object)) === selectorKey(selector));
      if (!object) return;
      setSelectedSchema(selectorKey(selector));
      setWorkspace("schema");
      if (object.root && (navigation || projection.pagesByNumber.has(object.root.pageNumber))) {
        setSelection({ type: "page", pageNumber: object.root.pageNumber });
      }
    } else {
      const page = projection.pagesByNumber.get(selector.pageNumber);
      if (navigation ? selector.pageNumber < 1 || selector.pageNumber > graph.coverage.evaluated
        : !page || (selector.type === "cell" && !page.detail.cells.some(cell => cell.identity.index === selector.cellIndex))) return;
      setSelection(selector);
    }
    navigation?.onSelect(selector);
    setEvidenceLocus(null);
  };
  const selectPage = (page: number) => selectEntity({ type: "page", pageNumber: page });
  const activeSchema = graph.schema.objects.find(object => selectorKey(schemaSelector(object)) === selectedSchema)
    ?? (navigation?.selectedSchema && selectorKey(schemaSelector(navigation.selectedSchema)) === selectedSchema ? navigation.selectedSchema : undefined);
  const attribution = navigation?.attribution ?? graph.schema.objects.filter(object => object.pages.some(page => page.pageNumber === selectedPage));

  return (
    <main className="workspace">
      <header className="topbar">
        <div>
          <p className="eyebrow">Volmap SQLite Inspector</p>
          <h1>{workspace === "atlas" ? "Page atlas" : "Schema flow"}</h1>
        </div>
        <div className="source-badge">
          <span className="status-dot" />
          <span>{source.displayName}</span>
        </div>
      </header>

      <nav className="workspace-tabs" aria-label="Inspection workspace">
        <button type="button" aria-pressed={workspace === "atlas"} onClick={() => setWorkspace("atlas")}>Page atlas</button>
        <button type="button" aria-pressed={workspace === "schema"} onClick={() => setWorkspace("schema")}>Schema flow</button>
      </nav>
      <label className="revision-picker">Inspection revision <select aria-label="Inspection revision" value={viewRevision ?? "latest"}
        onChange={event => onRevision(event.target.value === "latest" ? null : Number(event.target.value))}>
        <option value="latest">Latest revision ({status.revision ?? "none"})</option>
        {status.availableRevisions.map(revision => <option key={revision} value={revision}>Revision {revision}</option>)}
      </select></label>
      <InspectionNotice status={status} />
      {navigation?.controls}
      <Sidecars evidence={graph.sidecars} />

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
        {graph.semanticMetadata.coverage && <GeometryFact label="Helper coverage" value={`${graph.semanticMetadata.coverage.phase}: ${graph.semanticMetadata.coverage.reason}; ${graph.semanticMetadata.coverage.evaluatedBytes} / ${graph.semanticMetadata.coverage.totalBytes ?? "unknown"} bytes; remaining ${graph.semanticMetadata.coverage.remainderBytes ?? "unknown"}`} />}
        <GeometryFact label="B-tree path limit" value={graph.topologyCoverage.traversalBudget.maxBtreePages.toLocaleString()} />
        <GeometryFact label="Overflow path limit" value={graph.topologyCoverage.traversalBudget.maxOverflowPages.toLocaleString()} />
        <GeometryFact label="Aggregate traversal limit" value={BigInt(graph.topologyCoverage.traversalBudget.maxTotalPages).toLocaleString()} />
      </section>

      {workspace === "atlas" && <><Freelist evidence={graph.freelist} prefix={freelistTraversal?.validatedPrefix ?? []}
        firstNavigable={firstFreelistLink?.target.pageNumber ?? null} onSelectPage={selectPage} />
      <PointerMap evidence={graph.pointerMap} selectedPage={selectedPage} availablePages={navigation ? { has: number => number >= 1 && number <= graph.coverage.evaluated } : projection.pagesByNumber} onSelectPage={selectPage} /></>}
      <SchemaObjects evidence={graph.schema} selected={selectedSchema} onSelect={selectEntity} />
      <div className="content-grid">
        {workspace === "schema" ? <SchemaFlow graph={graph} object={activeSchema} selection={selection} onSelect={selectEntity} /> : <section className="atlas-panel">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Physical projection</p>
              <h2>{graph.coverage.reason === "complete" ? "Main-file mosaic" : "Inspected page prefix"}</h2>
            </div>
            <p>{graph.coverage.evaluated.toLocaleString()} inspected pages{navigation && ` · ${navigation.gridPages.size} shown`}</p>
          </div>
          <RoleLegend />
          <div className="mosaic">
            {graph.pages.filter(page => !navigation || navigation.gridPages.has(page.number)).map((page) => (
              <button
                className={page.number === selectedPage ? "page selected" : "page"}
                data-page-number={page.number}
                data-selector={selectorKey({ type: "page", pageNumber: page.number })}
                data-role={page.classification.role}
                data-finding={projection.findingsByPage.get(page.number) ?? "none"}
                aria-pressed={page.number === selectedPage}
                key={page.number}
                onClick={() => selectPage(page.number)}
                type="button"
              >
                <span>Page</span>
                <strong>{page.number}</strong>
                <span>{roleLabels[page.classification.role]}</span>
                {page.detail.coverage === "partial" && <span>Partial</span>}
                {projection.findingsByPage.has(page.number) && <span>{projection.findingsByPage.get(page.number) === "error" ? "Error" : "Warning"}</span>}
                {!page.classification.referenced && <span>Unreferenced</span>}
                {!page.classification.reconciled && <span>Not reconciled</span>}
              </button>
            ))}
          </div>
        </section>}

        <aside className="evidence-panel">
          <p className="eyebrow">Selection-linked evidence</p>
          <p>Selected evidence belongs to the physical main-file image. Sidecar changes are not applied.</p>
          <h2>{graph.pages.length ? `Page ${selectedPage}` : "No pages inspected"}</h2>
          {graph.pages.length > 0 && <>
          <dl>
            <div><dt>Identity</dt><dd>page:{selectedPage}</dd></div>
            <div><dt>Byte range</dt><dd>{((selectedPage - 1) * geometry.pageSize).toLocaleString()}–{(selectedPage * geometry.pageSize - 1).toLocaleString()}</dd></div>
            <div><dt>Role claim</dt><dd>{active ? roleLabels[active.classification.role] : "Unknown"}</dd></div>
          </dl>
          </>}
          <section aria-label="Schema attribution">
            <h3>Schema attribution</h3>
            {attribution.length ? <ul>{attribution.map(object => <li key={selectorKey(schemaSelector(object))}>
              <button type="button" onClick={() => selectEntity(schemaSelector(object))}>{object.objectType} {object.name}</button>
              {" · "}{object.state.replaceAll("_", " ")}
            </li>)}</ul> : <p>No validated schema attribution for this page.</p>}
          </section>
          <p className="evidence-note">
            Select a page to inspect its structural map and physical cell inventory below. Page roles reconcile physical evidence. Unknown pages retain opaque bytes; conflicts retain competing claims.
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
                selectPage(evidence.page.pageNumber);
                setEvidenceLocus(evidence);
              }}
            >
              Jump to page {evidence.page.pageNumber}, byte {evidence.range.pageOffset}
            </button>)}
          </li>;
        })}</ul>
      </section>}
      {activeTrunk && <section className="freelist-panel" aria-label="Freelist trunk evidence">
        <h2>Trunk {activeTrunk.page.pageNumber}</h2>
        <p>Leaf count: {activeTrunk.leafCount.value}</p>
        <p>Capacity: {activeTrunk.capacity} · Backward-compatible writer capacity: {activeTrunk.compatibilityCapacity}</p>
        <p>Leaf-count evidence: page bytes [4, 8), main-file bytes [{activeTrunk.leafCount.evidence.range.fileOffset}, {activeTrunk.leafCount.evidence.range.fileOffset + 4}).</p>
      </section>}
      {active && <RoleClaims classification={active.classification} onEvidence={evidence => {
        selectPage(evidence.page.pageNumber); setEvidenceLocus(evidence);
      }} />}
      {active && <PageDetail
        selectedCell={selection.type === "cell" ? selection.cellIndex : null}
        onSelectCell={index => selectEntity({ type: "cell", pageNumber: active.number, cellIndex: index })}
        detail={active.detail}
        pageSize={geometry.pageSize}
        pageNumber={active.number}
        claims={projection.claimsByPage.get(active.number) ?? []}
        relationshipsByClaim={projection.relationshipsByClaim}
        onSelectPage={selectPage}
        evidenceByte={evidenceLocus?.page.pageNumber === active.number ? evidenceLocus.range.pageOffset : null}
      />}
      {active && selection.type === "cell" && <DeepCell limits={status.deepLimits?.perJob} key={`${graph.snapshot.id}:${selection.pageNumber}:${selection.cellIndex}`}
        target={{ sessionId: status.sessionId, snapshotId: graph.snapshot.id, revision: graph.revision, pageNumber: selection.pageNumber, cellIndex: selection.cellIndex }}
        currentRevision={status.revision} onPublished={onRevision} /> }
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
      {progress.buildingSchema && ". Inspecting direct schema evidence."}
      {progress.buildingSidecars && ". Inspecting sidecar evidence."}
      {progress.buildingTopology && !progress.buildingSidecars && !progress.buildingSchema && ". Building bounded relationship topology."}
    </p>}
    {coverage && <p>
      {coverage.reason === "complete" ? "Page inventory complete" : "Partial coverage"}: {coverage.evaluated} pages evaluated.
      {" "}Remaining: {coverage.remainder ?? "unknown"}.
      {coverage.nextPage !== null && ` Stopped before page ${coverage.nextPage}.`}
      {" "}Reason: {coverage.reason.replaceAll("_", " ")}.
    </p>}
    {status.workProgress && <p>Work: {status.workProgress.phase.replaceAll("_", " ")} · {status.workProgress.evaluated} / {status.workProgress.total ?? "unknown"} units</p>}
    {status.operationalBudget && <details><summary>Effective inspection budgets</summary>
      <p>Resident memory: {status.operationalBudget.maxResidentBytes.toLocaleString()} bytes</p>
      <p>Processed cells: {status.operationalBudget.maxProcessedCells.toLocaleString()}</p>
      <p>Freelist trunk chain: {status.operationalBudget.maxFreelistTrunks?.toLocaleString() ?? "unknown"}</p>
      <p>Work units per phase: {status.operationalBudget.maxPhaseUnits.toLocaleString()}</p>
      {status.deepLimits && <>
        <p>Concurrent deep jobs: {status.deepLimits.maxConcurrentJobs}; retained jobs: {status.deepLimits.maxJobs}</p>
        <p>Reconstructed payload: {status.deepLimits.perJob.maxPayloadBytes.toLocaleString()} bytes; decoded values: {status.deepLimits.perJob.maxDecodedBytes.toLocaleString()} bytes</p>
        <p>Deep overflow pages: {status.deepLimits.perJob.maxOverflowPages.toLocaleString()}; value count: {status.deepLimits.perJob.maxValues.toLocaleString()}</p>
      </>}
      {status.traversalBudget && <p>B-tree depth: {status.traversalBudget.maxBtreePages}; overflow chain: {status.traversalBudget.maxOverflowPages}; aggregate traversal pages: {status.traversalBudget.maxTotalPages}</p>}
      {status.schemaBudget && <p>Schema decoding: {status.schemaBudget.maxDecodedBytes.toLocaleString()} bytes</p>}
      {status.sidecarBudget && <p>WAL frames: {status.sidecarBudget.maxWalFrames.toLocaleString()}</p>}
      {status.semanticBudget && <p>Metadata helper: {status.semanticBudget.maxCopyBytes.toLocaleString()} copied bytes; {status.semanticBudget.maxSchemaRecords} records; {status.semanticBudget.timeoutMs} ms; {status.semanticBudget.maxOutputBytes.toLocaleString()} output bytes</p>}
      {status.webLimits && <>
        <p>Request body: {status.webLimits.requestBytes.toLocaleString()} bytes; response: {status.webLimits.responseBytes.toLocaleString()} bytes</p>
        <p>Concurrent HTTP requests: {status.webLimits.concurrentRequests}; response collection items: {status.webLimits.collectionItems.toLocaleString()}</p>
      </>}
    </details>}
    {!!status.workCoverage?.length && <details><summary>Inspection work coverage</summary>
      {status.workCoverage.map((work, index) => <p key={index}>{work.phase.replaceAll("_", " ")}: {work.evaluated} / {work.total ?? "unknown"} units; next: {work.next ?? "none"}; remaining: {work.remainder ?? "unknown"}; {work.reason}{work.limit ? ` (${work.limit})` : ""}</p>)}
    </details>}
    {status.diagnostic && <p role="alert">{status.diagnostic.message}</p>}
    {status.diagnostic?.opaqueRange && <p>Opaque input: offset {status.diagnostic.opaqueRange.fileOffset}; length {status.diagnostic.opaqueRange.length} bytes</p>}
    {status.state === "invalidated" && <p>Navigation and further inspection are disabled. Open a new frozen copy to continue.</p>}
  </section>;
}

function GeometryFact({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

export function App() {
  const [viewRevision, setViewRevision] = useState<number | null>(null);
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
        if (response.status === 507) throw new Error("Web response limit reached. No partial inspection was returned.");
        if (!response.ok) throw new Error(`Inspection request failed (${response.status})`);
        let next: SessionStatus = await response.json();
        if (next.state === "invalidated" || next.revision === null) cached = null;
        else if (!next.storage && cached?.revision !== (viewRevision ?? next.revision)) {
          const revision = await fetch(`${base}/revisions/${viewRevision ?? next.revision}`, { cache: "no-store", signal: abort.signal });
          if (revision.status === 409) {
            next = await revision.json();
            cached = null;
          } else {
            if (revision.status === 507) throw new Error("Web response limit reached. No partial inspection was returned.");
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
  }, [viewRevision]);

  if (error) return <main className="message"><h1>Page atlas unavailable</h1><p>{error}</p></main>;
  if (!status) return <main className="message"><h1>Opening inspection…</h1></main>;
  if (status.storage && status.revision !== null && status.state !== "invalidated") return <WindowedAtlas status={status} revision={viewRevision ?? status.revision} viewRevision={viewRevision} onRevision={setViewRevision} />;
  if (graph) return <Atlas graph={graph} status={status} viewRevision={viewRevision} onRevision={setViewRevision} />;
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
