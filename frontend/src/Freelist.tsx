import type { PhysicalEvidence } from "./PageDetail";

interface Field { value: number; evidence: PhysicalEvidence }
export interface FreelistEvidence {
  firstTrunk: Field | null;
  declaredCount: Field | null;
  trunks: Array<{ page: { pageNumber: number }; leafCount: Field; capacity: number; compatibilityCapacity: number }>;
  coverage: { reason: "complete" | "invalid_structure" | "coverage_stop" | "budget" | "cancelled" | "operator_stop" | "not_inspected" | "unreadable"; stoppingClaim: string | null; evaluatedPages: number; remainder: number | null };
}

export function Freelist({ evidence, prefix, firstNavigable, onSelectPage }: {
  evidence: FreelistEvidence;
  prefix: Array<{ pageNumber: number }>;
  firstNavigable: number | null;
  onSelectPage: (page: number) => void;
}) {
  const { firstTrunk, declaredCount, coverage } = evidence;
  return <section className="freelist-panel" aria-label="Freelist allocation">
    <div className="section-heading">
      <h2>Freelist</h2>
      {firstNavigable !== null && <button type="button" onClick={() => onSelectPage(firstNavigable)}>Inspect freelist</button>}
    </div>
    <p>Declared free pages: {declaredCount?.value ?? "unknown"} · Evaluated: {coverage.evaluatedPages}</p>
    <p>Allocation coverage: {coverage.reason.replaceAll("_", " ")} · Remaining: {coverage.remainder ?? "unknown"}</p>
    <p>First trunk claim: {firstTrunk ? firstTrunk.value || "none" : "unknown"}.</p>
    {[firstTrunk, declaredCount].map((field, index) => field && <p key={index}>
      {index === 0 ? "First-trunk" : "Declared-count"} evidence: page {field.evidence.page.pageNumber}, bytes [{field.evidence.range.pageOffset}, {field.evidence.range.pageOffset + field.evidence.range.length}).
    </p>)}
    {coverage.stoppingClaim && <p>Stopping claim: {coverage.stoppingClaim}</p>}
    {coverage.reason === "complete" && firstTrunk?.value === 0 && <p>The freelist is empty.</p>}
    {prefix.length > 0 && <nav aria-label="Validated freelist trunk prefix">
      <span>Trunks: </span>{prefix.map(page => <button key={page.pageNumber} type="button" onClick={() => onSelectPage(page.pageNumber)}>Trunk {page.pageNumber}</button>)}
    </nav>}
  </section>;
}
