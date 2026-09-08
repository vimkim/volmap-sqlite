import type { EntityIdentity, PageIdentity, PhysicalEvidence, RelationshipClaim } from "./PageDetail";

export interface PointerMapEvidence {
  applicable: boolean;
  diagnostics: string[];
  complete: boolean;
  largestRoot: { value: number; evidence: PhysicalEvidence };
  incrementalVacuum: { value: number; evidence: PhysicalEvidence };
  locations: PageIdentity[];
  layout: { entriesPerPage: number; stride: number; displacedMap: PageIdentity | null; nextRegularAfterDisplaced: PageIdentity | null } | null;
  lockBytePage: PageIdentity | null;
  pages: Array<{
    page: PageIdentity; complete: boolean; diagnostics: string[];
    entries: Array<{ target: EntityIdentity; kind: string | null; rawKind: number; parentValue: number | null; parent: EntityIdentity | null; evidence: PhysicalEvidence; state: RelationshipClaim["state"]; diagnostics: string[] }>;
  }>;
}

export function PointerMap({ evidence, selectedPage, availablePages, onSelectPage }: {
  evidence: PointerMapEvidence; selectedPage: number; availablePages: ReadonlyMap<number, unknown>; onSelectPage: (page: number) => void;
}) {
  const active = evidence.pages.find(page => page.page.pageNumber === selectedPage);
  return <section className="page-detail" aria-label="Pointer-map evidence">
    <h2>Pointer maps and reserved pages</h2>
    <p>{evidence.applicable ? "Applicable" : "Not applicable"} · Largest root: {evidence.largestRoot.value} (header bytes 52–55) · Incremental vacuum: {evidence.incrementalVacuum.value} (header bytes 64–67)</p>
    {evidence.diagnostics?.length > 0 && <p>{evidence.diagnostics.map(code => code.replaceAll("_", " ")).join(" · ")}</p>}
    <p>Pointer-map coverage: {evidence.complete ? "complete" : "partial / not inspected"}</p>
    {evidence.layout && <p>{evidence.layout.entriesPerPage} entries per map · Map stride: {evidence.layout.stride} pages{evidence.layout.displacedMap && ` · Map displaced by lock-byte page: ${evidence.layout.displacedMap.pageNumber}`}</p>}
    <div className="map-links">{evidence.locations.map(page => <button key={page.pageNumber} type="button"
      disabled={!availablePages.has(page.pageNumber)} onClick={() => onSelectPage(page.pageNumber)}>
      Inspect pointer map page {page.pageNumber}
    </button>)}</div>
    <p>{evidence.lockBytePage ? <button type="button" disabled={!availablePages.has(evidence.lockBytePage.pageNumber)} onClick={() => onSelectPage(evidence.lockBytePage!.pageNumber)}>Inspect lock-byte page {evidence.lockBytePage.pageNumber}</button> : "No lock-byte page in this main-file image."}</p>
    {active && <>
      <h3>Pointer map page {active.page.pageNumber}</h3>
      <p>{active.complete ? "Complete readable entries" : "Partial readable entries"}{active.diagnostics.map(code => ` · ${code.replaceAll("_", " ")}`)}</p>
      <p>Target and parent buttons select physical evidence; a claimed relationship is supported only when its state is validated.</p>
      <div className="table-scroll"><table aria-label="Pointer-map entries"><thead><tr><th>Target</th><th>Entry kind</th><th>Parent / owner claim</th><th>Physical coordinates</th><th>State / findings</th></tr></thead>
        <tbody>{active.entries.map(entry => <tr key={entry.target.pageNumber}>
          <td>{availablePages.has(entry.target.pageNumber) ? <button type="button" onClick={() => onSelectPage(entry.target.pageNumber)}>Inspect mapped page {entry.target.pageNumber}</button> : `page:${entry.target.pageNumber} (outside inspected image)`}</td>
          <td>{entry.kind?.replaceAll("_", " ") ?? "Unsupported kind"} ({entry.rawKind})</td>
          <td>{entry.parent && availablePages.has(entry.parent.pageNumber) ? <button type="button" onClick={() => onSelectPage(entry.parent!.pageNumber)}>Inspect claimed parent page {entry.parent.pageNumber}</button> : entry.parentValue === null ? "Not readable" : entry.parentValue === 0 ? "None (0)" : `page:${entry.parentValue} (outside inspected image)`}</td>
          <td>Page bytes [{entry.evidence.range.pageOffset}, {entry.evidence.range.pageOffset + entry.evidence.range.length})<br />File bytes [{entry.evidence.range.fileOffset}, {entry.evidence.range.fileOffset + entry.evidence.range.length})</td>
          <td>{entry.state}{entry.diagnostics.map(code => <div key={code}>{code.replaceAll("_", " ")}</div>)}</td>
        </tr>)}</tbody>
      </table></div>
    </>}
  </section>;
}
