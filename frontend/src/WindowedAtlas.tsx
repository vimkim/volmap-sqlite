import { useEffect, useState, type ReactNode } from "react";
import { Atlas, type InspectionGraph, type SessionStatus } from "./App";
import type { EntitySelector, SchemaObject } from "./SchemaFlow";
import { TraversalWindow, type TraversalHeader } from "./TraversalWindow";

export interface WindowNavigation {
  selection: EntitySelector;
  gridPages: ReadonlySet<number>;
  attribution: SchemaObject[];
  selectedSchema?: SchemaObject;
  controls: ReactNode;
  onSelect: (selector: EntitySelector) => void;
}

interface Batch<T> { revision: number; offset: number; total: number; nextOffset: number | null; items: T[] }
interface Pages { revision: number; firstPage: number; total: number; nextPage: number | null; pages: InspectionGraph["pages"] }
interface Metadata {
  summary: Pick<InspectionGraph, "snapshot" | "revision" | "coverage" | "topologyCoverage"> & {
    pageCount: number; claimCount: number; relationshipCount: number; traversalCount: number;
    diagnosticCount: number; schemaObjectCount: number; freelistTrunkCount: number; pointerMapPageCount: number;
  };
  semanticMetadata: InspectionGraph["semanticMetadata"];
  deepInspections: InspectionGraph["deepInspections"];
  sidecars: InspectionGraph["sidecars"];
  schema: Omit<InspectionGraph["schema"], "objects">;
  freelist: Omit<InspectionGraph["freelist"], "trunks">;
  pointerMap: Omit<InspectionGraph["pointerMap"], "pages" | "locations">;
}
interface Collections {
  claims: InspectionGraph["relationshipClaims"][number];
  relationships: InspectionGraph["relationships"][number];
  traversals: TraversalHeader;
  diagnostics: InspectionGraph["diagnostics"][number];
  schema: Omit<SchemaObject, "pages">;
  schema_attribution: { objectOffset: number; object: Omit<SchemaObject, "pages"> };
  freelist_trunks: InspectionGraph["freelist"]["trunks"][number];
  pointer_maps: InspectionGraph["pointerMap"]["pages"][number];
}
type Kind = keyof Collections;
type Offsets = Record<Kind, number>;
type Batches = { [K in Kind]: Batch<Collections[K]> };
const initialOffsets: Offsets = { claims: 0, relationships: 0, traversals: 0, diagnostics: 0, schema: 0, schema_attribution: 0, freelist_trunks: 0, pointer_maps: 0 };
const labels: Record<Kind, string> = {
  claims: "Selected-page claims", relationships: "Selected-page relationships", traversals: "Selected-page traversals",
  diagnostics: "Diagnostics", schema: "Schema objects", freelist_trunks: "Freelist trunks", pointer_maps: "Pointer maps",
  schema_attribution: "Selected-page schema objects",
};
const collectionKinds: Kind[] = ["claims", "relationships", "traversals", "diagnostics", "schema", "schema_attribution", "freelist_trunks", "pointer_maps"];
interface View { selectedPage: number; selectedSchema?: SchemaObject; graph: InspectionGraph; pages: Pages; batches: Batches; attribution: Batch<{ pageNumber: number }> | null }

async function get<T>(url: string, signal: AbortSignal): Promise<T> {
  const response = await fetch(url, { cache: "no-store", signal });
  if (response.status === 409) throw new Error("Snapshot invalidated; navigation is withheld.");
  if (response.status === 507) throw new Error("This evidence window exceeds the response or memory budget. Choose a smaller window or another entity.");
  if (!response.ok) throw new Error(`Evidence unavailable (${response.status}).`);
  return response.json();
}

async function loadView(base: string, page: number, first: number, offsets: Offsets, object: number | null, attributionOffset: number, size: number, signal: AbortSignal): Promise<View> {
  const metadata = await get<Metadata>(`${base}/metadata`, signal);
  const { summary } = metadata;
  const pages = await get<Pages>(`${base}/pages/${first}/${size}`, signal);
  const selected = page <= summary.pageCount && !pages.pages.some(item => item.number === page)
    ? (await get<Pages>(`${base}/pages/${page}/1`, signal)).pages : [];
  async function collection<K extends Kind>(kind: K, total: number, local = false): Promise<Batch<Collections[K]>> {
    if (total === 0 || (local && summary.pageCount === 0)) return { revision: summary.revision, offset: offsets[kind], total: 0, nextOffset: null, items: [] };
    const prefix = local ? `/pages/${page}` : "";
    const wireKind = kind === "schema_attribution" ? "schema" : kind === "traversals" ? "traversal_headers" : kind;
    return get<Batch<Collections[K]>>(`${base}${prefix}/collections/${wireKind}/${offsets[kind]}/${size}`, signal);
  }
  // Responses are retained only for these windows; requests are sequential to bound server work.
  const batches: Batches = {
    claims: await collection("claims", summary.claimCount, true),
    relationships: await collection("relationships", summary.relationshipCount, true),
    traversals: await collection("traversals", summary.traversalCount, true),
    diagnostics: await collection("diagnostics", summary.diagnosticCount),
    schema: await collection("schema", summary.schemaObjectCount),
    schema_attribution: await collection("schema_attribution", summary.schemaObjectCount, true),
    freelist_trunks: await collection("freelist_trunks", summary.freelistTrunkCount),
    pointer_maps: await collection("pointer_maps", summary.pointerMapPageCount),
  };
  const attribution = object === null ? null : await get<Batch<{ pageNumber: number }>>(`${base}/schema/${object}/pages/${attributionOffset}/${size}`, signal);
  const selectedHeader = object === null ? undefined : batches.schema.items[object - batches.schema.offset]
    ?? (await get<Batch<Collections["schema"]>>(`${base}/collections/schema/${object}/1`, signal)).items[0];
  const map = summary.pointerMapPageCount && summary.pageCount ? await get<Collections["pointer_maps"] | null>(`${base}/pages/${page}/allocation/pointer_map`, signal) : null;
  const trunk = summary.freelistTrunkCount && summary.pageCount ? await get<Collections["freelist_trunks"] | null>(`${base}/pages/${page}/allocation/freelist_trunk`, signal) : null;
  const maps = map && !batches.pointer_maps.items.some(item => item.page.pageNumber === page) ? [...batches.pointer_maps.items, map] : batches.pointer_maps.items;
  const trunks = trunk && !batches.freelist_trunks.items.some(item => item.page.pageNumber === page) ? [...batches.freelist_trunks.items, trunk] : batches.freelist_trunks.items;
  return {
    selectedPage: page, selectedSchema: selectedHeader ? { ...selectedHeader, pages: attribution?.items ?? [] } : undefined, pages, batches, attribution,
    graph: {
      ...summary, semanticMetadata: metadata.semanticMetadata, deepInspections: metadata.deepInspections, sidecars: metadata.sidecars,
      pages: [...pages.pages, ...selected], relationshipClaims: batches.claims.items, relationships: batches.relationships.items,
      traversals: [], diagnostics: batches.diagnostics.items,
      schema: { ...metadata.schema, objects: batches.schema.items.map((header, index) => ({ ...header, pages: batches.schema.offset + index === object ? attribution?.items ?? [] : [] })) },
      freelist: { ...metadata.freelist, trunks },
      pointerMap: { ...metadata.pointerMap, pages: maps, locations: batches.pointer_maps.items.map(item => item.page) },
    },
  };
}

function Pager({ label, offset, count, total, next, onOffset, size }: { label: string; offset: number; count: number; total: number; next: number | null; onOffset: (offset: number) => void; size: number }) {
  return <div className="collection-pager" aria-label={`${label} window`}>
    <span>{label}: {count ? `${offset + 1}–${offset + count}` : "0"} of {total.toLocaleString()}</span>{" "}
    <button type="button" disabled={offset === 0} onClick={() => onOffset(Math.max(0, offset - size))} aria-label={`Previous ${label.toLowerCase()}`}>Previous</button>{" "}
    <button type="button" disabled={next === null} onClick={() => next !== null && onOffset(next)} aria-label={`Next ${label.toLowerCase()}`}>Next</button>
  </div>;
}

export function WindowedAtlas({ status, revision, viewRevision, onRevision }: { status: SessionStatus; revision: number; viewRevision: number | null; onRevision: (revision: number | null) => void }) {
  const [size, setSize] = useState(32);
  const [selection, setSelection] = useState<EntitySelector>({ type: "page", pageNumber: 1 });
  const [first, setFirst] = useState(1);
  const [offsets, setOffsets] = useState(initialOffsets);
  const [object, setObject] = useState<number | null>(null);
  const [attributionOffset, setAttributionOffset] = useState(0);
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [jump, setJump] = useState("");
  const [retry, setRetry] = useState(0);
  const [traversal, setTraversal] = useState<TraversalHeader | null>(null);
  const page = selection.pageNumber;
  useEffect(() => {
    const abort = new AbortController();
    setBusy(true); setError(null);
    const base = `/api/snapshots/${encodeURIComponent(status.snapshotId)}/revisions/${revision}`;
    void loadView(base, page, first, offsets, object, attributionOffset, size, abort.signal).then(next => {
      if (!abort.signal.aborted) setView(next);
    }).catch((reason: unknown) => {
      if (!abort.signal.aborted) { setView(null); setError(reason instanceof Error ? reason.message : "Evidence unavailable."); }
    }).finally(() => { if (!abort.signal.aborted) setBusy(false); });
    return () => abort.abort();
  }, [status.snapshotId, revision, page, first, offsets, object, attributionOffset, size, retry]);

  const select = (selector: EntitySelector) => {
    if (selector.type === "cell" && selector.pageNumber === page) { setSelection(selector); return; }
    setTraversal(null);
    if (selector.type === "schema") {
      const index = view?.batches.schema.items.findIndex(item => item.identity.pageNumber === selector.pageNumber && item.identity.index === selector.cellIndex) ?? -1;
      const attributed = view?.batches.schema_attribution.items.find(item => item.object.identity.pageNumber === selector.pageNumber && item.object.identity.index === selector.cellIndex);
      if (!view || (index < 0 && !attributed)) return;
      const position = index < 0 ? attributed!.objectOffset : view.batches.schema.offset + index;
      const header = index < 0 ? attributed!.object : view.batches.schema.items[index];
      setObject(position); setAttributionOffset(0);
      setOffsets(prior => ({ ...prior, schema: Math.floor(position / size) * size }));
      const root = header.root;
      if (root) { setSelection({ type: "page", pageNumber: root.pageNumber }); setFirst(root.pageNumber); }
    } else {
      setSelection(selector);
      if (!view?.pages.pages.some(item => item.number === selector.pageNumber)) setFirst(selector.pageNumber);
    }
    setOffsets(prior => ({ ...prior, claims: 0, relationships: 0, traversals: 0, schema_attribution: 0 }));
  };
  const sizeControl = <label>Records per window <select aria-label="Records per window" value={size} onChange={event => {
    setSize(Number(event.target.value)); setOffsets(initialOffsets); setAttributionOffset(0);
  }}>{[1, 8, 16, 32].map(count => <option key={count} value={count}>{count}</option>)}</select></label>;
  if (!view) return <main className="message"><h1>{error ? "Evidence window unavailable" : "Loading revision evidence…"}</h1>{error && <><p>{error}</p>{sizeControl}<button type="button" onClick={() => setRetry(prior => prior + 1)}>Retry evidence window</button></>}</main>;
  const controls = <section aria-label="Evidence windows">
    <h2>Browse complete collections</h2>
    {sizeControl}
    <p>Each range is a window into the retained revision. Coverage above describes the inspection; the ranges below describe what is displayed.</p>
    <p role="status">{busy ? "Loading evidence window…" : "Evidence window ready."}</p>
    <form onSubmit={event => { event.preventDefault(); const number = Number(jump); if (Number.isInteger(number) && number >= 1 && number <= view.graph.coverage.evaluated) select({ type: "page", pageNumber: number }); }}>
      <label>Go to page <input aria-label="Go to page" type="number" min={1} max={view.graph.coverage.evaluated} value={jump} onChange={event => setJump(event.target.value)} /></label>{" "}<button type="submit">Go</button>
    </form>
    <Pager size={size} label="Atlas pages" offset={view.pages.firstPage - 1} count={view.pages.pages.length} total={view.pages.total} next={view.pages.nextPage === null ? null : view.pages.nextPage - 1} onOffset={offset => setFirst(offset + 1)} />
    {collectionKinds.map(kind => <Pager size={size} key={kind} label={labels[kind]} offset={view.batches[kind].offset} count={view.batches[kind].items.length} total={view.batches[kind].total} next={view.batches[kind].nextOffset} onOffset={offset => setOffsets(prior => ({ ...prior, [kind]: offset }))} />)}
    {view.attribution && <Pager size={size} label="Attributed pages" offset={view.attribution.offset} count={view.attribution.items.length} total={view.attribution.total} next={view.attribution.nextOffset} onOffset={setAttributionOffset} />}
    <details><summary>Displayed traversal and relationship records</summary>
      {view.batches.relationships.items.map(link => <p key={link.claimId}>{link.kind}: <button type="button" onClick={() => select({ type: "page", pageNumber: link.source.pageNumber })}>Page {link.source.pageNumber}</button> → <button type="button" onClick={() => select({ type: "page", pageNumber: link.target.pageNumber })}>Page {link.target.pageNumber}</button></p>)}
      {view.batches.traversals.items.map(header => <p key={header.traversalOffset}>{header.kind} from page {header.origin.pageNumber} · {header.prefixCount.toLocaleString()} prefix pages · {header.stop?.reason ?? "complete"}{" "}<button type="button" aria-label={`Inspect ${header.kind} traversal ${header.traversalOffset}`} onClick={() => setTraversal(header)}>Inspect prefix</button></p>)}
      {view.batches.freelist_trunks.items.map(trunk => <p key={trunk.page.pageNumber}><button type="button" onClick={() => select({ type: "page", pageNumber: trunk.page.pageNumber })}>Freelist trunk {trunk.page.pageNumber}</button> · {trunk.leafCount.value} leaves</p>)}
    </details>
    {traversal && <TraversalWindow key={`${revision}:${traversal.traversalOffset}:${size}`} base={`/api/snapshots/${encodeURIComponent(status.snapshotId)}/revisions/${revision}`} header={traversal} size={size} onSelect={pageNumber => select({ type: "page", pageNumber })} />}
    {status.storage && <p>Retained cache: {status.storage.cacheBytes.toLocaleString()} B · Private spill: {status.storage.spillBytes.toLocaleString()} B · Spilled indexes: {status.storage.spilledIndexes}</p>}
  </section>;
  const loadingRevision = view.graph.revision !== revision;
  return <>{loadingRevision && <main className="message"><h1>Loading revision evidence…</h1></main>}
    <div hidden={loadingRevision}><Atlas loadingRevision={loadingRevision} graph={view.graph} status={status} viewRevision={viewRevision} onRevision={onRevision} navigation={{ selectedSchema: view.selectedSchema, selection: page === view.selectedPage ? selection : { type: "page", pageNumber: view.selectedPage }, attribution: view.batches.schema_attribution.items.map(item => ({ ...item.object, pages: [{ pageNumber: view.selectedPage }] })), gridPages: new Set(view.pages.pages.map(page => page.number)), controls, onSelect: select }} /></div></>;
}
