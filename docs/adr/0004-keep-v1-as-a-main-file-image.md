# Keep v1 as a main-file image

Version 1 presents the physical main-file image and automatically discovers adjacent WAL, rollback-journal, and shared-memory sidecars only to disclose their presence and supported metadata. It does not overlay WAL frames, perform virtual rollback, or treat shared memory as authoritative, because doing so would turn a physical inspector into a transaction-state interpreter; the UI must therefore never claim that a main-file image with sidecars is the latest logical database image.
