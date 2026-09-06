import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

type TextEncoding = "utf8" | "utf16_le" | "utf16_be";

interface InspectionGraph {
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
  pages: Array<{ number: number }>;
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

function Atlas({ graph }: { graph: InspectionGraph }) {
  const { geometry, source } = graph.snapshot;
  const [selectedPage, setSelectedPage] = useState(1);

  return (
    <main className="workspace">
      <header className="topbar">
        <div>
          <p className="eyebrow">Volmap SQLite Inspector</p>
          <h1>Page atlas</h1>
        </div>
        <div className="source-badge">
          <span className="status-dot" />
          <span>{source.displayName}</span>
        </div>
      </header>

      <section className="geometry" aria-label="Snapshot geometry">
        <GeometryFact label="Pages" value={geometry.pageCount.toLocaleString()} />
        <GeometryFact label="Page size" value={`${geometry.pageSize.toLocaleString()} B`} />
        <GeometryFact label="Usable size" value={`${geometry.usableSize.toLocaleString()} B`} />
        <GeometryFact label="Reserved" value={`${geometry.reservedBytes} B`} />
        <GeometryFact label="Encoding" value={encodingLabel[geometry.textEncoding]} />
      </section>

      <div className="content-grid">
        <section className="atlas-panel">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Physical projection</p>
              <h2>Complete main-file mosaic</h2>
            </div>
            <p>{graph.pages.length.toLocaleString()} complete pages</p>
          </div>
          <div className="mosaic">
            {graph.pages.map((page) => (
              <button
                className={page.number === selectedPage ? "page selected" : "page"}
                data-page-number={page.number}
                key={page.number}
                onClick={() => setSelectedPage(page.number)}
                type="button"
              >
                <span>Page</span>
                <strong>{page.number}</strong>
              </button>
            ))}
          </div>
        </section>

        <aside className="evidence-panel">
          <p className="eyebrow">Selection-linked evidence</p>
          <h2>Page {selectedPage}</h2>
          <dl>
            <div><dt>Identity</dt><dd>page:{selectedPage}</dd></div>
            <div><dt>Byte range</dt><dd>{((selectedPage - 1) * geometry.pageSize).toLocaleString()}–{(selectedPage * geometry.pageSize - 1).toLocaleString()}</dd></div>
            <div><dt>Role</dt><dd className="unknown">Not inspected</dd></div>
          </dl>
          <p className="evidence-note">
            This tracer inspection establishes geometry and physical page identity. Structural roles arrive in the next inspection stage.
          </p>
        </aside>
      </div>
    </main>
  );
}

function GeometryFact({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function App() {
  const [graph, setGraph] = useState<InspectionGraph | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const snapshotId = encodeURIComponent(window.__VOLMAP_BOOTSTRAP__.snapshotId);
    fetch(`/api/snapshots/${snapshotId}`, { cache: "no-store" })
      .then((response) => {
        if (!response.ok) throw new Error(`Inspection request failed (${response.status})`);
        return response.json() as Promise<InspectionGraph>;
      })
      .then(setGraph)
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : "Inspection request failed"));
  }, []);

  if (error) return <main className="message"><h1>Page atlas unavailable</h1><p>{error}</p></main>;
  if (!graph) return <main className="message"><p className="eyebrow">Volmap SQLite Inspector</p><h1>Opening page atlas…</h1></main>;
  return <Atlas graph={graph} />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode><App /></StrictMode>,
);
