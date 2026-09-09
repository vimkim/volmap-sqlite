export interface ByteRange { pageOffset: number; fileOffset: number; length: number }
export interface PageIdentity { pageNumber: number }
export type EntityIdentity =
  | { type: "page"; pageNumber: number }
  | { type: "cell"; pageNumber: number; cellIndex: number };
export interface PhysicalEvidence {
  page: PageIdentity;
  range: ByteRange;
  validationRule: string;
}
export interface RelationshipClaim {
  id: string;
  kind: "btree_child" | "overflow" | "freelist_trunk" | "freelist_leaf";
  source: EntityIdentity;
  target: PageIdentity | null;
  evidence: PhysicalEvidence;
  state: "validated" | "unresolved" | "invalid" | "conflicting" | "terminal";
}
export interface Relationship {
  claimId: string;
  kind: "btree_child" | "overflow" | "freelist_trunk" | "freelist_leaf";
  source: EntityIdentity;
  target: PageIdentity;
}
export interface StructuralDiagnostic {
  code: string;
  severity: "warning" | "error";
  evidence: PhysicalEvidence[];
  affectedRelationships: string[];
  containment: "traversal_stopped" | "relationship_excluded";
}
export interface PageEvidence {
  allocationRole: "freelist_trunk" | "freelist_leaf" | "conflicting" | null;
  kind: "table_leaf" | "table_interior" | "index_leaf" | "index_interior" | null;
  coverage: "complete" | "partial" | "unsupported";
  header: { range: ByteRange; firstFreeblock: number; cellCount: number; contentStart: number; fragmentedBytes: number; rightmostChild: number | null } | null;
  regions: Array<{ kind: string; range: ByteRange }>;
  cells: Array<{
    identity: { pageNumber: number; index: number }; pointer: ByteRange; offset: number;
    range: ByteRange | null; rowid: string | null; leftChild: number | null;
    leftChildPointer: ByteRange | null;
    payloadSize: number | null; localPayload: ByteRange | null; overflowPage: number | null;
    overflowPointer: ByteRange | null;
    record: { state: "complete" | "invalid" | "needs_overflow" | "unsupported_format"; headerSize: number | null; serialTypes: string[] } | null;
    diagnostic: string | null;
  }>;
  freeblocks: Array<{ offset: number; next: number; range: ByteRange | null; diagnostic: string | null }>;
  diagnostics: string[];
}

export function roleLabel(kind: PageEvidence["kind"] | PageEvidence["allocationRole"]): string {
  return kind ? kind.replaceAll("_", " ") : "Unknown / opaque";
}

function extent(range: ByteRange | null, file = false): string {
  if (!range) return "Not validated";
  const start = file ? range.fileOffset : range.pageOffset;
  return `[${start}, ${start + range.length})`;
}

function sourcePage(source: EntityIdentity): number {
  return source.pageNumber;
}

function relationshipLabel(kind: Relationship["kind"]): string {
  return kind.replaceAll("_", " ");
}

export function PageDetail({
  detail, pageSize, pageNumber, claims, relationshipsByClaim, onSelectPage, evidenceByte, selectedCell, onSelectCell,
}: {
  selectedCell: number | null;
  onSelectCell: (index: number) => void;
  detail: PageEvidence;
  pageSize: number;
  pageNumber: number;
  claims: RelationshipClaim[];
  relationshipsByClaim: ReadonlyMap<string, Relationship>;
  onSelectPage: (page: number) => void;
  evidenceByte: number | null;
}) {
  const { header } = detail;
  const allocations = [
    ...detail.regions.filter(region => region.kind !== "usable_space" && region.kind !== "cell_content"),
    ...detail.cells.flatMap(cell => cell.range ? [{ kind: `cell:${cell.identity.pageNumber}:${cell.identity.index}`, range: cell.range }] : []),
    ...detail.freeblocks.flatMap(block => block.range ? [{ kind: "freeblock", range: block.range }] : []),
  ];
  return <section className="page-detail" aria-label="Page structural detail">
    <div className="section-heading"><h2>Structural evidence</h2><span>Local B-tree coverage: {detail.coverage}</span></div>
    <p className="evidence-note">Local structural observations and relationship claims are preserved alongside the reconciled page role. Application values and raw payload bytes are not disclosed.</p>
    {evidenceByte !== null && <p className="evidence-locus" role="status">Diagnostic evidence locus: page {pageNumber}, byte {evidenceByte}</p>}
    {detail.diagnostics.length > 0 && <ul className="diagnostics">{detail.diagnostics.map((code, index) => <li key={index}>{code.replaceAll("_", " ")}</li>)}</ul>}
    {header && <dl className="header-fields">
      <div><dt>Header (page bytes)</dt><dd>{extent(header.range)}</dd></div>
      <div><dt>Header (file bytes)</dt><dd>{extent(header.range, true)}</dd></div>
      <div><dt>Cells</dt><dd>{header.cellCount}</dd></div>
      <div><dt>Content start</dt><dd>{header.contentStart}</dd></div>
      <div><dt>First freeblock</dt><dd>{header.firstFreeblock || "None"}</dd></div>
      <div><dt>Fragment bytes (declared)</dt><dd>{header.fragmentedBytes}</dd></div>
      {header.rightmostChild !== null && <div><dt>Rightmost child claim</dt><dd>{header.rightmostChild}</dd></div>}
    </dl>}
    <section aria-label="Structural byte map">
      <h3>Page-relative byte map</h3>
      <p className="evidence-note">Zero-based bytes; all ranges are [start, end), excluding end. Unclassified gaps remain blank. Summary regions can contain other regions.</p>
      <div className="byte-map" aria-label={`Page bytes 0 to ${pageSize}`}>
        {allocations.filter(({ range }) => range.length > 0).map(({ kind, range }, index) => <span
          key={index} className={`byte-segment ${kind.startsWith("cell:") ? "cell" : kind}`}
          style={{ left: `${100 * range.pageOffset / pageSize}%`, width: `${100 * range.length / pageSize}%` }}
          title={`${kind}: ${extent(range)} · ${range.length} B`} />)}
      </div>
      <div className="table-scroll"><table aria-label="Byte regions"><thead><tr><th>Region</th><th>Page bytes</th><th>Main-file bytes</th><th>Size</th></tr></thead>
        <tbody>{detail.regions.map(({ kind, range }, index) => <tr key={index}><td>{kind.replaceAll("_", " ")}</td><td>{extent(range)}</td><td>{extent(range, true)}</td><td>{range.length} B</td></tr>)}</tbody>
      </table></div>
    </section>
    <section aria-label="Page relationships">
      <h3>Relationship claims</h3>
      {claims.length === 0 ? <p className="evidence-note">No structural relationship claims touch this page.</p> :
      <div className="table-scroll"><table aria-label="Relationship claims"><thead><tr><th>Relationship</th><th>Direction</th><th>Target</th><th>Evidence</th><th>Validation</th></tr></thead>
        <tbody>{claims.map(claim => {
          const outgoing = sourcePage(claim.source) === pageNumber;
          const relationship = relationshipsByClaim.get(claim.id);
          const destination = outgoing ? relationship?.target.pageNumber : relationship ? sourcePage(relationship.source) : undefined;
          return <tr key={claim.id}>
            <td>{claim.id}</td>
            <td>{outgoing ? "Outgoing" : "Incoming"}</td>
            <td>{claim.target ? `page:${claim.target.pageNumber}` : "End of chain"}</td>
            <td>Page bytes {extent(claim.evidence.range)}<br />{claim.evidence.validationRule.replaceAll("_", " ")}</td>
            <td><span className={`claim-state ${claim.state}`}>{claim.state}</span>
              {destination !== undefined && destination !== pageNumber && <><br /><button type="button" onClick={() => onSelectPage(destination)}
                aria-label={`Follow ${relationshipLabel(claim.kind)} ${outgoing ? "to" : "back to"} page ${destination}`}>
                {outgoing ? "Follow target" : "Follow source"}
              </button></>}
            </td>
          </tr>;
        })}</tbody>
      </table></div>}
    </section>
    {detail.allocationRole === "freelist_leaf" && <p>Freelist leaf contents are unused; no cells are interpreted.</p>}
    <h3>Cell inventory</h3>
    {selectedCell !== null && <p role="status" aria-label="Selected cell">Selected cell: cell:{pageNumber}:{selectedCell} · Structural evidence only.</p>}
    <p className="evidence-note">Identity uses the zero-based cell-pointer-array index, not the rowid or key. Child and overflow pointers link to the validated relationship evidence above.</p>
    <div className="table-scroll"><table aria-label="Cell inventory"><thead><tr><th>Physical identity</th><th>Pointer / offset</th><th>Validated extent</th><th>Structural facts</th><th>Record structure</th></tr></thead>
      <tbody>{detail.cells.map(cell => <tr key={cell.identity.index} aria-selected={selectedCell === cell.identity.index}>
        <td><button type="button" aria-label={`Select cell ${cell.identity.pageNumber}:${cell.identity.index}`} onClick={() => onSelectCell(cell.identity.index)}>{`cell:${cell.identity.pageNumber}:${cell.identity.index}`}</button></td>
        <td>{extent(cell.pointer)} → {cell.offset}</td>
        <td>{extent(cell.range)}{cell.range && <><br />File {extent(cell.range, true)}<br />{cell.range.length} B</>}</td>
        <td>{cell.rowid !== null && <>Rowid: <span>{cell.rowid}</span><br /></>}
          {cell.leftChild !== null && <>Left child: {cell.leftChild}<br /></>}
          {cell.payloadSize !== null && <>Payload: {cell.payloadSize} B<br />Local: {extent(cell.localPayload)}<br /></>}
          {cell.overflowPage !== null && <>Overflow: {cell.overflowPage}</>}
        </td>
        <td>{cell.diagnostic && <p className="diagnostics">{cell.diagnostic.replaceAll("_", " ")}</p>}
          {cell.record && <><span>{cell.record.state.replaceAll("_", " ")}</span><br />Header: {cell.record.headerSize ?? "unknown"} B<br />Serial types: {cell.record.serialTypes.join(", ") || "none available"}</>}
        </td>
      </tr>)}</tbody>
    </table></div>
    {detail.cells.length === 0 && <p>No structurally available cells.</p>}
    {detail.freeblocks.length > 0 && <><h3>Freeblocks</h3><div className="table-scroll"><table aria-label="Freeblock chain"><thead><tr><th>Offset</th><th>Validated page extent</th><th>Next claim</th><th>Diagnostic</th></tr></thead><tbody>
      {detail.freeblocks.map(block => <tr key={block.offset}><td>{block.offset}</td><td>{extent(block.range)}</td><td>{block.next || "End"}</td><td>{block.diagnostic?.replaceAll("_", " ") ?? "—"}</td></tr>)}
    </tbody></table></div></>}
  </section>;
}
