import { useEffect, useRef, useState } from "react";
import type { PhysicalEvidence } from "./PageDetail";

interface Target { sessionId: string; snapshotId: string; revision: number; pageNumber: number; cellIndex: number }
export interface Budget { maxPayloadBytes: number; maxOverflowPages: number; maxValues: number; maxDecodedBytes: number }
interface Coverage {
  phase: string; reason: string; payloadBytes: string | null; reconstructedBytes: string;
  remainderBytes: string | null; decodedValues: number; decodedBytes?: string; expectedValues: number | null;
  stoppingPage: number | null; stoppingPayloadOffset: string | null;
  evidence: PhysicalEvidence[]; overflowPages: { pageNumber: number }[];
}
interface Job {
  id: string; target: Target; state: "pending" | "completed" | "cancelled" | "budget_stopped" | "invalid_target" | "stale_revision" | "invalidated_snapshot" | "failed";
  coverage: Coverage; resultRevision: number | null;
}
interface Field { ordinal: number; serialType: string; payloadOffset: string; byteLength: string; source: PhysicalEvidence[]; columnName: string | null }
export interface DeepEvidence { cell: { pageNumber: number; index: number }; fields: Field[]; coverage: Coverage; columnMetadata: string }
type Value = { type: "null" } | { type: "blob"; byteLength: string } | { type: "text" | "integer" | "real"; value: string };
interface Result { target: Target; revision: number; evidence: DeepEvidence; values: { field: Field; value: Value }[] }

const defaults: Budget = { maxPayloadBytes: 16777216, maxOverflowPages: 32768, maxValues: 4096, maxDecodedBytes: 16777216 };
const limitLabels: Record<keyof Budget, string> = { maxPayloadBytes: "Payload byte limit", maxOverflowPages: "Overflow page limit", maxValues: "Value count limit", maxDecodedBytes: "Decoded byte limit" };
function sameTarget(a: Target, b: Target) {
  return a.sessionId === b.sessionId && a.snapshotId === b.snapshotId && a.revision === b.revision && a.pageNumber === b.pageNumber && a.cellIndex === b.cellIndex;
}

export function DeepCell({ target, currentRevision, onPublished, limits, withheld = false }: { withheld?: boolean; limits?: Budget; target: Target; currentRevision: number | null; onPublished: (revision: number) => void }) {
  const [budget, setBudget] = useState(limits ?? defaults);
  const [request, setRequest] = useState<{ target: Target; budget: Budget } | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const active = useRef<AbortController | null>(null);
  const base = `/api/snapshots/${encodeURIComponent(target.snapshotId)}/deep-inspections`;
  useEffect(() => {
    if (!request) return;
    const abort = new AbortController(); active.current = abort;
    let timer: ReturnType<typeof setTimeout>;
    const read = async (url: string, body?: unknown) => {
      const response = await fetch(url, { cache: "no-store", signal: abort.signal,
        ...(body === undefined ? {} : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }) });
      if (!response.ok) throw new Error("Deep inspection response unavailable. Refresh inspection status before retrying.");
      return response.json();
    };
    const accept = async (next: Job) => {
      if (abort.signal.aborted) return;
      if (!sameTarget(next.target, request.target)) throw new Error("Deep inspection target mismatch.");
      setJob(next);
      if (next.state === "pending") {
        timer = setTimeout(() => { void poll(next.id); }, 150);
      } else if (next.state === "completed") {
        const value: Result = await read(`${base}/${encodeURIComponent(next.id)}/result`, request.target);
        if (abort.signal.aborted) return;
        if (!sameTarget(value.target, request.target) || value.revision !== next.resultRevision) throw new Error("Deep result scope mismatch.");
        setResult(value); onPublished(value.revision);
      }
    };
    const fail = () => { if (!abort.signal.aborted) setError("Deep inspection unavailable. No values were displayed; inspect session status before retrying."); };
    const poll = async (id: string) => { try { await accept(await read(`${base}/${encodeURIComponent(id)}`)); } catch { fail(); } };
    void read(base, request).then(accept).catch(fail);
    return () => { abort.abort(); clearTimeout(timer); };
  }, [request, base, onPublished]);
  const pending = request !== null && (job === null || job.state === "pending") && error === null;
  const validBudget = Object.entries(budget).every(([key, value]) => Number.isSafeInteger(value) && value >= 0 && (!limits || value <= limits[key as keyof Budget]) && (key === "maxPayloadBytes" || key === "maxDecodedBytes" || value <= 4294967295));
  const visible = !withheld && result?.revision === target.revision && result.target.pageNumber === target.pageNumber && result.target.cellIndex === target.cellIndex;
  return <section className="deep-cell" aria-label="Selected-cell deep inspection">
    <h2>Deep inspection · cell:{target.pageNumber}:{target.cellIndex}</h2>
    <p>Decode this cell's stored values. BLOBs show their type and length. A successful inspection publishes a new immutable revision.</p>
    <details><summary>Deep inspection limits</summary>{(Object.keys(budget) as (keyof Budget)[]).map(key => <label key={key}>
      {limitLabels[key]} <input type="number" min="0" step="1" value={budget[key]} max={limits?.[key]} disabled={pending}
        onChange={event => setBudget(previous => ({ ...previous, [key]: Number(event.target.value) }))} />
    </label>)}</details>
    {target.revision !== currentRevision && <p>Viewing a historical revision. Choose the latest revision before starting new work.</p>}
    <button type="button" disabled={withheld || pending || !validBudget || target.revision !== currentRevision} onClick={() => {
      setJob(null); setResult(null); setError(null); setCancelling(false); setRequest({ target: { ...target }, budget: { ...budget } });
    }}>Deep-inspect selected cell</button>
    {pending && job && <button type="button" disabled={cancelling} onClick={() => {
      setCancelling(true);
      void fetch(`${base}/${encodeURIComponent(job.id)}/cancel`, { method: "POST", cache: "no-store", signal: active.current?.signal })
        .then(response => { if (!response.ok) throw new Error(); })
        .catch(() => { if (!active.current?.signal.aborted) setError("Cancellation request failed; job status remains available."); });
    }}>Cancel deep inspection</button>}
    {(job || pending) && <p role="status">Deep inspection: {job?.state.replaceAll("_", " ") ?? "pending"}</p>}
    {job && <div className="deep-coverage">
      <p>{job.coverage.phase} · {job.coverage.reason.replaceAll("_", " ")} · Reconstructed {job.coverage.reconstructedBytes} / {job.coverage.payloadBytes ?? "unknown"} bytes · Remaining {job.coverage.remainderBytes ?? "unknown"}</p>
      <p>Decoded bytes: {job.coverage.decodedBytes ?? "unknown"}</p>
      <p>Decoded fields: {job.coverage.decodedValues} / {job.coverage.expectedValues ?? "unknown"}</p>
      {job.coverage.stoppingPayloadOffset !== null && <p>Stopped at payload offset {job.coverage.stoppingPayloadOffset}, page {job.coverage.stoppingPage ?? "unknown"}.</p>}
      <p>Overflow prefix: {job.coverage.overflowPages.map(page => page.pageNumber).join(" → ") || "none"}</p>
    </div>}
    {error && <p role="alert">{error}</p>}
    {visible && result && <section aria-label="Selected stored values">
      <h3>Stored values · revision {result.revision}</h3>
      <p>Column metadata: {result.evidence.columnMetadata}. Ordinals describe the physical record; rowid aliases are not substituted.</p>
      <table><thead><tr><th>Field</th><th>Type / serial type</th><th>Stored value</th><th>Provenance</th></tr></thead>
        <tbody>{result.values.map(({ field, value }) => <tr key={field.ordinal}>
          <td>{field.columnName ?? `Field ${field.ordinal}`}</td><td>{value.type} / {field.serialType}</td>
          <td className="stored-value">{value.type === "null" ? "NULL" : value.type === "blob" ? `BLOB · ${value.byteLength} bytes` : value.value}</td>
          <td>Payload {field.payloadOffset} + {field.byteLength} bytes
            {field.source.map((source, index) => <div key={index}>Page {source.page.pageNumber} [{source.range.pageOffset}, {source.range.pageOffset + source.range.length})</div>)}
          </td>
        </tr>)}</tbody></table>
    </section>}
  </section>;
}
