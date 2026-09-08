export interface ByteRange { pageOffset: number; fileOffset: number; length: number }
export interface PageEvidence {
  kind: "table_leaf" | "table_interior" | "index_leaf" | "index_interior" | null;
  coverage: "complete" | "partial" | "unsupported";
  header: { range: ByteRange; firstFreeblock: number; cellCount: number; contentStart: number; fragmentedBytes: number; rightmostChild: number | null } | null;
  regions: Array<{ kind: string; range: ByteRange }>;
  cells: Array<{
    identity: { pageNumber: number; index: number }; pointer: ByteRange; offset: number;
    range: ByteRange | null; rowid: string | null; leftChild: number | null;
    payloadSize: number | null; localPayload: ByteRange | null; overflowPage: number | null;
    record: { state: "complete" | "invalid" | "needs_overflow" | "unsupported_format"; headerSize: number | null; serialTypes: string[] } | null;
    diagnostic: string | null;
  }>;
  freeblocks: Array<{ offset: number; next: number; range: ByteRange | null; diagnostic: string | null }>;
  diagnostics: string[];
}

export function roleLabel(kind: PageEvidence["kind"]): string {
  return kind ? kind.replaceAll("_", " ") : "Unknown / opaque";
}

function extent(range: ByteRange | null, file = false): string {
  if (!range) return "Not validated";
  const start = file ? range.fileOffset : range.pageOffset;
  return `[${start}, ${start + range.length})`;
}

export function PageDetail({ detail, pageSize }: { detail: PageEvidence; pageSize: number }) {
  const { header } = detail;
  const allocations = [
    ...detail.regions.filter(region => region.kind !== "usable_space" && region.kind !== "cell_content"),
    ...detail.cells.flatMap(cell => cell.range ? [{ kind: `cell:${cell.identity.pageNumber}:${cell.identity.index}`, range: cell.range }] : []),
    ...detail.freeblocks.flatMap(block => block.range ? [{ kind: "freeblock", range: block.range }] : []),
  ];
  return <section className="page-detail" aria-label="Page structural detail">
    <div className="section-heading"><h2>Structural evidence</h2><span>Local B-tree coverage: {detail.coverage}</span></div>
    <p className="evidence-note">Role is a locally validated header claim; topology and global role reconciliation are not evaluated. Application values and raw payload bytes are not disclosed.</p>
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
    <h3>Cell inventory</h3>
    <p className="evidence-note">Identity uses the zero-based cell-pointer-array index, not the rowid or key. Child and overflow numbers are untraversed claims.</p>
    <div className="table-scroll"><table aria-label="Cell inventory"><thead><tr><th>Physical identity</th><th>Pointer / offset</th><th>Validated extent</th><th>Structural facts</th><th>Record structure</th></tr></thead>
      <tbody>{detail.cells.map(cell => <tr key={cell.identity.index}>
        <td>{`cell:${cell.identity.pageNumber}:${cell.identity.index}`}</td>
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
