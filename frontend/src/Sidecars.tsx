import { useState } from "react";

export interface SidecarEvidence {
  kind: "wal" | "journal" | "shm";
  displayName: string;
  length: string;
  state: string;
  consequence: "none" | "wal_not_applied" | "rollback_not_applied" | "shm_non_authoritative";
  fields: Array<{ name: string; value: number; offset: string; length: number }>;
  diagnostics: Array<{ code: string; offset: string; length: string }>;
  coverage: { scope: string; reason: string; evaluatedBytes: string; remainingBytes: string };
  wal: {
    frameLimit: string;
    checksumByteOrder: string;
    validatedFrames: string;
    lastCommitFrame: string | null;
    frames: Array<{ number: string; offset: string; readableBytes: number; pageNumber: number | null; databaseSize: number | null; salts: [number, number] | null; storedChecksum: [number, number] | null; checksumValid: boolean | null; state: string; diagnostic: string | null }>;
  } | null;
  journal: { headerValid: boolean; exceedsHotSize: boolean; hotStatus: string; reservedLock: string; superJournal: string } | null;
  shm: { byteOrder: string; copiesMatch: boolean | null; headerChecksumsValid: boolean | null; maxFrame: number | null; backfill: number | null } | null;
}

const consequences = {
  none: "No adjacent candidate observed.",
  wal_not_applied: "WAL not applied. Committed changes may be absent from the main-file image.",
  rollback_not_applied: "Rollback not applied. The main-file image may contain changes that ordinary SQLite opening would roll back.",
  shm_non_authoritative: "SHM is non-authoritative coordination and lookup evidence; it is not required for recovery.",
};
const label = (value: string) => value.replaceAll("_", " ");
const check = (value: boolean | null) => value === null ? "unknown" : value ? "yes" : "no";

function Sidecar({ evidence }: { evidence: SidecarEvidence }) {
  const [expanded, setExpanded] = useState(false);
  const [frameStart, setFrameStart] = useState(0);
  const { wal, journal, shm } = evidence;
  return <article className="sidecar">
    <h3>{evidence.kind.toUpperCase()}: {evidence.state === "absent" ? "absent" : evidence.displayName}</h3>
    <p>{consequences[evidence.consequence]}</p>
    {evidence.state !== "absent" && <>
      <p>{label(evidence.state)} · {evidence.length} bytes · {label(evidence.coverage.scope)}: {label(evidence.coverage.reason)}</p>
      <p>{evidence.coverage.evaluatedBytes} bytes evaluated; {evidence.coverage.remainingBytes} bytes outside the evaluated extent.</p>
      {evidence.state === "unresolved_tail" && <p>Frame salts differ from the WAL header. The tail may belong to an earlier WAL generation; this boundary alone does not establish corruption.</p>}
      {wal && <p>Checksum byte order: {wal.checksumByteOrder}. Frame limit: {wal.frameLimit}. Validated frame prefix: {wal.validatedFrames}. Last validated commit boundary: {wal.lastCommitFrame ?? "none"}. Later frames are not established as committed.</p>}
      {journal && <p>Header valid: {check(journal.headerValid)}. Larger than 512 bytes: {check(journal.exceedsHotSize)}. Hot-journal status: {label(journal.hotStatus)}. Reserved lock: {label(journal.reservedLock)}. Super-journal: {label(journal.superJournal)}. Recovery eligibility is not established; records are not replayed or validated.</p>}
      {shm && <p>Byte order: {label(shm.byteOrder)} (producer byte order is not portable). Header copies match: {check(shm.copiesMatch)}. Header checksums valid: {check(shm.headerChecksumsValid)}. Frame count claim: {shm.maxFrame ?? "unknown"}; backfill claim: {shm.backfill ?? "unknown"}. These claims do not establish main-file or WAL state. Hash tables and live lock ownership are not interpreted.</p>}
      {evidence.diagnostics.length > 0 && <ul>{evidence.diagnostics.map((finding, index) => <li key={index}>{finding.code} · sidecar offset {finding.offset}, length {finding.length}</li>)}</ul>}
      <button type="button" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? "Hide" : "Inspect"} {evidence.kind.toUpperCase()} metadata</button>
      {expanded && <>
        <table><caption>{evidence.kind.toUpperCase()} header evidence</caption><thead><tr><th>Field</th><th>Value</th><th>Sidecar offset</th><th>Bytes</th></tr></thead><tbody>{evidence.fields.map(field => <tr key={`${field.offset}:${field.name}`}><td>{label(field.name)}</td><td>{field.value}</td><td>{field.offset}</td><td>{field.length}</td></tr>)}</tbody></table>
        {wal && wal.frames.length > 0 && <>
          <p>Frames {frameStart + 1}–{Math.min(frameStart + 50, wal.frames.length)} of {wal.frames.length} observed</p>
          <button type="button" disabled={frameStart === 0} onClick={() => setFrameStart(Math.max(0, frameStart - 50))}>Previous frames</button>{" "}
          <button type="button" disabled={frameStart + 50 >= wal.frames.length} onClick={() => setFrameStart(frameStart + 50)}>Next frames</button>
          <div className="sidecar-frames"><table><caption>WAL frame evidence (sidecar references)</caption><thead><tr><th>Frame</th><th>Offset / bytes</th><th>Page claim</th><th>Commit size</th><th>Salts</th><th>Stored checksum</th><th>Checksum valid</th><th>Validation</th></tr></thead><tbody>{wal.frames.slice(frameStart, frameStart + 50).map(frame => <tr key={frame.number}><td>{frame.number}</td><td>{frame.offset} / {frame.readableBytes}</td><td>{frame.pageNumber ?? "unreadable"}</td><td>{frame.databaseSize === 0 ? "no marker" : frame.databaseSize ?? "unreadable"}</td><td>{frame.salts?.join(", ") ?? "unreadable"}</td><td>{frame.storedChecksum?.join(", ") ?? "unreadable"}</td><td>{check(frame.checksumValid)}</td><td>{frame.state}{frame.diagnostic && `: ${frame.diagnostic}`}</td></tr>)}</tbody></table></div>
        </>}
      </>}
    </>}
  </article>;
}

export function Sidecars({ evidence }: { evidence: SidecarEvidence[] }) {
  return <section className="sidecar-disclosure" aria-label="Sidecar evidence">
    <h2>Physical main-file image</h2>
    <p>Pages show the accepted main-file bytes. Sidecars are inspected separately and never applied.</p>
    <div className="sidecar-list">{evidence.map(sidecar => <Sidecar key={sidecar.kind} evidence={sidecar} />)}</div>
  </section>;
}
