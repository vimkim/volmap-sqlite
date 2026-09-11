# v1 accepted

On 2026-09-11 the user reported trying the Inspector, then explicitly accepted it:
“I think it's nice. I accept.” This records product acceptance; it does not claim
the user individually executed every automated scenario.

Accepted source baseline: `ee5e94c` (ticket 16 plus the requested remote-serving
justfile defaults). All 16 implementation tickets are now closed. Tickets 1–4 had
stale unchecked source drafts despite their implementation commits and current
regression coverage; tickets 10–12 had completed implementation/checklists but
stale `ready-for-human` labels. The original specification and previously untracked
ticket drafts are included in the closeout commit so the tracker is durable.

The accepted v1 effort retains package version **0.1.0**. No version bump is needed
to record acceptance, and no public release or Git tag is created by this closeout.

## Evidence

[Release validation](VALIDATION.md) records 181 passing Rust tests, the separate
semantic-helper checks, 29 passing frontend tests, formatting and Clippy, copied
executable browser/terminal/security checks, and extended 2 GiB inspection. Two
independent clean builds produced identical executables and build manifests.
Two existing test entries are ignored by the normal suite as documented there.

The subsequent remote-serving recipe change was checked through a non-loopback
network interface, with the remote warning and explicit address overrides verified.
The production/build inputs are unchanged from the verified artifact; the later
changes are developer helpers, documentation, and tracker records.

Executable SHA-256:
`ee311a34fa99422e48c03b78ab63a5a5d3208941c7b60927aa0d04afe2faa02c`.

Content build identity:
`75c909d6727ee60287510d2d3ca735b7cf939ccf66010350a69c6ff38fb987fa`.

## Local release package

`target/release-package/volmap-sqlite-0.1.0-linux-x86_64.tar.gz` contains the verified
executable, standalone release notes, build and verification manifests, and member
checksums. An adjacent `.sha256` file checks the archive itself. Packaging verifies
the source executable against the recorded manifest and then verifies every member
after extraction. The executable is reused from the two-build verification rather
than rebuilt for documentation-only changes.

[Release notes](RELEASE_NOTES.md) include local, remote, and terminal commands.
The package is prepared locally; publication remains a separate action.

Archive SHA-256:
`fa882f433dc8cfe4d08bc17a5ac26719dc84ed3966f5e2240df250ee35e0a26a`.

All archive member checksums and the extracted executable's version matched during
closeout. The production suite was not rerun for these documentation/tracker-only
changes; the verified executable itself is unchanged.
