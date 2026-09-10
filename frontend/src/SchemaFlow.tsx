import type { InspectionGraph } from "./App";
import type { PageIdentity, PhysicalEvidence } from "./PageDetail";

export type EntitySelector =
  | { type: "page"; pageNumber: number }
  | { type: "cell"; pageNumber: number; cellIndex: number }
  | { type: "schema"; pageNumber: number; cellIndex: number };

export function selectorKey(selector: EntitySelector): string {
  return selector.type === "page" ? `page:${selector.pageNumber}`
    : `${selector.type}:${selector.pageNumber}:${selector.cellIndex}`;
}

export interface SchemaObject {
  identity: { pageNumber: number; index: number };
  evidence: PhysicalEvidence[];
  objectType: string | null;
  name: string | null;
  tableName: string | null;
  rootPage: string | null;
  declaration: string | null;
  root: PageIdentity | null;
  pages: PageIdentity[];
  state: "complete" | "partial" | "unavailable" | "declaration_only";
  diagnostics: string[];
}

export interface SchemaEvidence {
  objects: SchemaObject[];
  state: "complete" | "partial" | "unavailable";
  diagnostics: string[];
  maxDecodedBytes: string;
  decodedBytes: string;
  stoppingCell: SchemaObject["identity"] | null;
}

export function schemaSelector(object: SchemaObject): EntitySelector {
  return { type: "schema", pageNumber: object.identity.pageNumber, cellIndex: object.identity.index };
}

export function SchemaObjects({ evidence, selected, onSelect }: {
  evidence: SchemaEvidence; selected: string | null; onSelect: (selector: EntitySelector) => void;
}) {
  return <section className="schema-objects" aria-label="Schema objects">
    <h2>Schema objects</h2>
    <p>Direct schema coverage: {evidence.state}. Decoded {evidence.decodedBytes} / {evidence.maxDecodedBytes} payload bytes.</p>
    {evidence.diagnostics.map(code => <p key={code}>{code.replaceAll("_", " ")}</p>)}
    {evidence.stoppingCell && <p>Stopped at schema cell {evidence.stoppingCell.pageNumber}:{evidence.stoppingCell.index}; remaining declarations unknown.</p>}
    {evidence.objects.length === 0 && <p>{evidence.state === "complete" ? "No stored schema declarations." : "Schema declarations unavailable in the inspected prefix."}</p>}
    <ul>{evidence.objects.map(object => {
      const selector = schemaSelector(object);
      const key = selectorKey(selector);
      return <li key={key}><button type="button" data-selector={key} aria-pressed={selected === key}
        aria-label={`Inspect schema ${object.objectType ?? "record"} ${object.name ?? key}`}
        onClick={() => onSelect(selector)}>
        <strong>{object.name ?? "Unavailable schema record"}</strong>
        <span>{object.objectType ?? "unknown"} · {object.state.replaceAll("_", " ")} · {key}</span>
      </button></li>;
    })}</ul>
  </section>;
}

export function SchemaFlow({ graph, object, selection, onSelect }: {
  graph: InspectionGraph; object: SchemaObject | undefined; selection: EntitySelector; onSelect: (selector: EntitySelector) => void;
}) {
  if (!object) return <section className="schema-flow"><h2>Semantic projection</h2><p>Select a schema object to follow its storage evidence.</p></section>;
  const description = graph.semanticMetadata?.tables.find(table => table.identity.pageNumber === object.identity.pageNumber
    && table.identity.index === object.identity.index);
  const pages = new Set(object.pages.map(page => page.pageNumber));
  const selectedPage = pages.has(selection.pageNumber) ? graph.pages.find(page => page.number === selection.pageNumber) : undefined;
  const parents = new Map(graph.relationships.filter(link => link.kind === "btree_child"
    && pages.has(link.source.pageNumber) && pages.has(link.target.pageNumber))
    .map(link => [link.target.pageNumber, link.source.pageNumber]));
  return <section className="schema-flow" aria-label="Schema storage flow">
    <div className="schema-declaration">
      <p className="eyebrow">1 · Schema declaration</p>
      <h2>{object.name ?? "Unavailable schema record"}</h2>
      <p>{object.objectType ?? "unknown"} · Related table: {object.tableName ?? "unavailable"}</p>
      <p>State: {object.state.replaceAll("_", " ")} · Root claim: {object.rootPage ?? "NULL"}</p>
      {object.declaration !== null ? <pre>{object.declaration}</pre> : <p>No declaration text available (implicit indexes may store NULL).</p>}
      <p aria-label="Descriptive semantic metadata">{description
        ? `Descriptive SQLite metadata: ${description.columnCount} declared columns · ${description.strict ? "STRICT" : "non-STRICT"} · ${description.withoutRowid ? "WITHOUT ROWID" : "rowid table"}.`
        : "Descriptive SQLite metadata unavailable."}</p>
      {object.diagnostics.map(code => <p key={code} className="diagnostics">{code.replaceAll("_", " ")}</p>)}
      <details><summary>Physical schema record evidence</summary>
        {object.evidence.map((evidence, index) => <p key={index}>
          <button type="button" onClick={() => onSelect({ type: "page", pageNumber: evidence.page.pageNumber })}>Schema evidence page {evidence.page.pageNumber}</button>
          {" "}bytes [{evidence.range.pageOffset}, {evidence.range.pageOffset + evidence.range.length}) · {evidence.validationRule}
        </p>)}
      </details>
    </div>
    {object.state === "declaration_only" ? <div className="schema-termination"><h3>Declaration only</h3><p>Declaration only — no directly attributable storage B-tree.</p></div>
      : !object.root ? <div className="schema-termination"><h3>Attribution unavailable</h3><p>The root claim does not establish a validated storage path. Physical evidence remains available.</p></div>
      : <>
        <div className="schema-root"><p className="eyebrow">2 · Validated root B-tree</p>
          <button type="button" onClick={() => onSelect({ type: "page", pageNumber: object.root!.pageNumber })}>Root B-tree page {object.root.pageNumber}</button>
          <p>↓ Follow validated B-tree relationships</p>
        </div>
        <div className="schema-descendants"><p className="eyebrow">3 · Pages → physical cells</p>
          {object.state === "partial" && <p>Partial attribution: only validated traversal prefixes are shown.</p>}
          <ul>{object.pages.map(page => {
            const parent = parents.get(page.pageNumber);
            return <li key={page.pageNumber}>
              {parent === undefined ? <button type="button" onClick={() => onSelect({ type: "page", pageNumber: page.pageNumber })}>{page.pageNumber === object.root?.pageNumber ? "Root" : "Attributed"} page {page.pageNumber}</button> : <button type="button"
                onClick={() => onSelect({ type: "page", pageNumber: page.pageNumber })}>
                Descendant page {page.pageNumber} from page {parent}
              </button>}
              <span> → select this page to inspect and select its physical cells below.</span>
            </li>;
          })}</ul>
        </div>
        <div className="schema-cells"><p className="eyebrow">4 · Selected page → cells</p>
          {selectedPage ? <>
            <h3>Cells on attributed page {selectedPage.number}</h3>
            <div className="map-links">{selectedPage.detail.cells.filter(cell => cell.range !== null).map(cell => {
              const selector: EntitySelector = { type: "cell", pageNumber: selectedPage.number, cellIndex: cell.identity.index };
              return <button type="button" key={selectorKey(selector)} data-selector={selectorKey(selector)}
                aria-pressed={selectorKey(selection) === selectorKey(selector)}
                onClick={() => onSelect(selector)}>Select attributed cell {selectedPage.number}:{cell.identity.index}</button>;
            })}</div>
            {!selectedPage.detail.cells.some(cell => cell.range !== null) && <p>No cells with validated extents on this page.</p>}
          </> : <p>Select an attributed page to follow its physical cells.</p>}
        </div>
      </>}
  </section>;
}
