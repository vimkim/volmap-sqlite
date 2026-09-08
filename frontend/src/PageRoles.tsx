import type { PhysicalEvidence, RelationshipClaim } from "./PageDetail";

export const roleLabels = {
  btree: "B-tree (subtype unknown)", table_leaf: "table leaf", table_interior: "table interior",
  index_leaf: "index leaf", index_interior: "index interior", freelist: "Freelist (subtype unknown)",
  freelist_trunk: "freelist trunk", freelist_leaf: "freelist leaf", overflow: "overflow",
  pointer_map: "pointer map", lock_byte: "lock byte", unknown: "Unknown / opaque", conflicting: "conflicting",
} as const;
export type PageRole = keyof typeof roleLabels;
export interface Classification {
  role: PageRole;
  referenced: boolean;
  reconciled: boolean;
  claims: Array<{ role: PageRole; source: string; state: RelationshipClaim["state"]; evidence: PhysicalEvidence; relationshipId: string | null }>;
}

export function RoleLegend() {
  return <section className="role-legend" aria-label="Role and finding legend">
    <h3>Role and finding legend</h3>
    <ul>{Object.entries(roleLabels).map(([role, label]) => <li data-role={role} key={role}>{label}</li>)}</ul>
    <p>Unreferenced: no relationship claim names this page. Warning / Error: evidence-backed findings.
      Partial: incomplete local structure. Not reconciled: the global role pass did not reach this page.
      Unknown pages retain readable opaque evidence; conflicting pages retain competing claims.</p>
  </section>;
}

export function RoleClaims({ classification, onEvidence }: { classification: Classification; onEvidence: (evidence: PhysicalEvidence) => void }) {
  return <section className="page-detail" aria-label="Page role evidence">
    <h2>Page role evidence</h2>
    <p>{roleLabels[classification.role]} · {classification.reconciled ? "Reconciled" : "Not reconciled"} · {classification.referenced ? "Referenced" : "Unreferenced"}</p>
    {classification.claims.length === 0 ? <p>No validated supported role; readable bytes remain opaque evidence.</p> :
      <div className="table-scroll"><table aria-label="Page role claims">
        <thead><tr><th>Role</th><th>Source</th><th>State</th><th>Physical evidence</th></tr></thead>
        <tbody>{classification.claims.map((claim, index) => <tr key={index}>
          <td>{roleLabels[claim.role]}</td><td>{claim.source.replaceAll("_", " ")}</td><td>{claim.state}</td>
          <td><button type="button" onClick={() => onEvidence(claim.evidence)}>Inspect role evidence on page {claim.evidence.page.pageNumber}</button><br />
            Page bytes [{claim.evidence.range.pageOffset}, {claim.evidence.range.pageOffset + claim.evidence.range.length}), file bytes [{claim.evidence.range.fileOffset}, {claim.evidence.range.fileOffset + claim.evidence.range.length})<br />
            {claim.evidence.validationRule.replaceAll("_", " ")}</td>
        </tr>)}</tbody>
      </table></div>}
  </section>;
}
