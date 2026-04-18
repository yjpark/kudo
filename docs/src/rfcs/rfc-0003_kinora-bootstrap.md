# RFC-0003: Kinora Bootstrap

- **Status**: Accepted
- **Created**: 2026-04-10
- **Updated**: 2026-04-18
- **Project**: Kudo (edger-dev/kudo)
- **Components**: Kinora
- **Implementation**: [edger-dev/kinora](https://github.com/edger-dev/kinora) — bootstrap tracked in bean `kinora-w7w0`; decisions in `kinora-fhw1`

## Summary

Kinora can be bootstrapped as files in a Git repository before any application code exists. Since Kinora's design positions files in Git as the source of truth — with all other representations (indices, rendered views, search) as derived tooling — there is nothing preventing the system from operating as a manually-managed file structure with minimal CLI tooling. This RFC describes the bootstrap approach: a content-addressed file store, an append-only ledger, and a small CLI that enforces the invariants.

Kinora lives as a `.kinora/` directory within the project repository. Its lifecycle states map directly to Git's own state machine, and a separate cache layer (outside the repo) provides cross-branch visibility through rendered output.

## Motivation

The Kudo project is generating design knowledge through exploratory discussions — conversations about data models, architectural boundaries, filtering strategies, and system principles. This knowledge needs to be captured in a way that:

- Preserves the append-only, versioned nature of evolving ideas (an early design note is not "wrong" when thinking evolves — it is a historical version)
- Supports composition (assembling related kinos into coherent topic views without folder hierarchies)
- Tests Kinora's own design principles before any Kinora application code is written
- Remains useful as plain files in Git even if tooling changes completely

The bootstrap approach treats the documentation process itself as a tracer bullet for Kinora's knowledge management design.

## Design

### Repository Layout

Kinora data lives inside the project repository as a `.kinora/` directory:

```
.kinora/
  store/
    ab/
      ab3f7c...  (content blob)
    d1/
      d1e92a...  (content blob)
  ledger
```

This eliminates the need for a separate repository. Each Git branch carries its own `.kinora/` state, and cross-branch visibility is handled by the CLI's rendering layer (see Rendering below).

### Kino Lifecycle

A kino's lifecycle maps directly to Git's state machine:

| Git state   | Kinora state   | What it means |
|-------------|----------------|---------------|
| untracked   | **sketch**     | Content exists as a file on disk, not yet processed by Kinora. The user is still writing. |
| staged      | **staged**     | `store` has hashed the content, placed it in the content store, and appended a ledger entry. The changes are in the working tree (and may be `git add`'d) but not yet git-committed. |
| committed   | **committed**  | Git-committed. The content hash, ledger entry, and provenance are immutable in history. |

By tying Kinora's states to Git's, there is no separate commit mechanism to learn or maintain. The `store` CLI command is the only Kinora-specific action; after that, normal `git add` and `git commit` complete the lifecycle.

**Draft is a separate dimension.** A committed kino can carry a `draft` flag in its ledger metadata, indicating the content is still evolving and not yet authoritative. Draft is orthogonal to lifecycle state — a kino can be committed (immutable in Git history) while still being a draft (not yet the final word on its topic). Superseding a draft means appending a new ledger entry for the next version.

### Content Store

All content is stored by hash. When a kino is created, its content is hashed, and the file is stored at a path derived from that hash.

The path structure uses the first two characters of the hash as a directory prefix (following Git's convention) to avoid filesystem performance issues with large flat directories.

Content files are pure content — no metadata, no frontmatter, no headers added by the system. The content you put in is exactly the content stored. This means the hash is deterministic and verifiable: anyone can hash the file and confirm it matches its path.

A human never directly creates or names a file in the content store. Content enters only through a tool that hashes and places it.

### Ledger

The ledger is an append-only file (or set of files) that records every meaningful event:

- A new kino version was created (name, hash, author, provenance, timestamp, draft flag)
- A kinograph was created or updated (name, list of kino references, author, timestamp)

The ledger is how you answer questions like "what is the latest version of the filtering-boundary kino?" or "who authored this content and where did it come from?" The content store alone cannot answer these questions — it only knows hashes and content.

The ledger is append-only: entries are never modified or deleted. To supersede a kino version, you append a new entry pointing to new content. The old entry and old content remain.

### Kinographs

A kinograph is a composition — a file that references specific content hashes to assemble a coherent view of a topic. A kinograph about "Moco Data Model" might reference the content hashes of kinos about content addressing, the reference layer, deduplication, and derived data.

Kinographs are themselves stored in the content store (they are content, after all) and tracked in the ledger. Updating a kinograph means creating a new version with updated references.

### Provenance

Every ledger entry includes provenance: who created this content and where it came from. For the bootstrap phase, provenance is simple text — "derived from Claude conversation on 2026-04-10" or "authored by YJ after prototype experiment." This tests Kinora's mandatory provenance principle with zero tooling complexity.

### Rendering

The `render` command scans all local branches and worktrees, reads each branch's `.kinora/` ledger and store, and produces a merged mdbook site. The output includes:

- A `SUMMARY.md` file assembling kinographs into a readable structure, organized by branch
- Rendered markdown pages with source path markers showing the original content-addressed file path and originating branch
- Kino versions from all branches, so the reader can see what's happening across parallel work streams

The rendered output lives outside the repository at `~/.cache/kinora/<project>/`, keeping it cleanly separated from source data. This cache is rebuilt on demand — run `render` and it regenerates everything from scratch. No watchers or incremental updates in the bootstrap phase.

This rendering cache is the prototype for Kinora's future persistent cache layer. The structure and content of the rendered output define what the cache will eventually maintain incrementally. By building it as a full rebuild first, the design of the cache emerges from actual usage rather than upfront speculation.

### Beans Sync

Actionable items identified in kinographs can be exported to beans task specs. A generator reads kinographs, identifies items tagged or structured as tasks, and produces a spec file that beans can consume. The kinograph remains the source of truth; the beans spec is a derived artifact, regenerated on demand.

This follows the same pattern as mdbook rendering: Kinora data is the source, everything else is a view.

## Minimal CLI

The bootstrap requires a small CLI that enforces content addressing and ledger invariants. Three commands are sufficient to start:

### `store`

Takes content (from stdin or a file path), hashes it, writes it to the content store at the hash-derived path, and appends a ledger entry with metadata (kino name, author, provenance, draft flag, timestamp). This is the single transition from sketch to staged — after this, normal Git workflow (`git add`, `git commit`) completes the lifecycle.

If the content already exists in the store (same hash), the store is not modified, but a new ledger entry is still appended (because a new version of a named kino might point to previously-existing content).

### `resolve`

Given a kino name, reads the ledger to find the latest entry for that name and returns the content from the store. Can also resolve a specific version (by timestamp or version number) rather than latest.

### `render`

Scans all local branches and worktrees, resolves all referenced hashes from each branch's `.kinora/`, and produces mdbook-compatible output at `~/.cache/kinora/<project>/` — SUMMARY.md and assembled markdown files with source markers and branch annotations.

## Bootstrap Sequence

1. Create the `.kinora/` directory in the kudo repository with the content store and ledger structure
2. Implement the minimal CLI (store, resolve, render)
3. Migrate the RFC documents into the content store as the first kinos
4. Create kinographs that compose these kinos into topic views
5. Set up mdbook rendering to `~/.cache/kinora/kudo/` as the first derived view
6. Use the system going forward for all Kudo design documentation

The RFCs themselves (including this one) become the first content managed by the system they describe.

## Design Principles

1. **Files in Git are the source of truth** — All other representations (rendered sites, search indices, task specs) are derived and regeneratable. If the tooling breaks, the data survives as plain files in Git.

2. **Content addressing is enforced, not conventional** — The CLI enforces that content enters the store by hash. This is not a convention that humans maintain — it is a mechanical invariant.

3. **Append-only by structure** — The ledger only grows. Kino versions are never overwritten. This property is enforced by the data layout, not by discipline.

4. **Provenance is mandatory** — Every ledger entry records who created the content and where it came from. There is no anonymous content.

5. **Rendering is always derived** — mdbook output, beans specs, and any future views are generated from the source data. They are never edited directly.

6. **Lifecycle mirrors Git** — Kinora does not invent its own commit mechanism. The kino lifecycle (sketch → staged → committed) maps directly to Git's state machine (untracked → staged → committed), and draft status is an orthogonal metadata dimension.

## Resolved Decisions

The open questions raised during drafting were resolved through a collaborative brainstorming session. Each decision is recorded below in summary; the full rationale lives in the kinora repo as bean [`kinora-fhw1`](https://github.com/edger-dev/kinora).

### Hash algorithm

**BLAKE3**, plain hash mode (unkeyed), 32-byte output encoded as 64-char lowercase hex. Content store path shards by first 2 hex chars: `.kinora/store/aa/aabb…`.

*Why:* Stable public spec, first-class Rust support, WASM/no-std friendly, optimized for tree hashing. Since Kinora ships its own CLI, `sha256sum` ubiquity is less critical.

### Ledger format

**JSONL** on disk — one JSON event per line, append-only. **facet** (not serde) for internal serialization.

File layout is a directory of per-lineage files under `.kinora/ledger/`, with content-addressed filenames (`<shorthash>.jsonl` = first 8 hex of BLAKE3 of that lineage's first event). No privileged trunk. First `store` on a new git branch mints a new lineage file; `.kinora/HEAD` tracks the current lineage.

*Why:* Line-oriented format fits append-only and is universally recoverable. Per-lineage files make git merges structurally conflict-free (all merges are additive file changes). Content-addressed filenames match Kinora's ethos and avoid branch-name coupling.

### Kinograph format and kino model

A kino is a triple: **identity** (immutable BLAKE3 of first content version), **version** (content hash), **metadata** (latest-wins per field). Name is metadata, not identity — no uniqueness enforcement.

The version DAG uses `parents[]` that may cross identities, unifying linear history, forks, detach (new id with cross-identity parent), and combine (new id with multiple cross-identity parents) without new event kinds.

Kinographs are styx documents with an `entries[]` list; each entry is `{id, name?, pin?, note?}` — id authoritative, name a non-authoritative hint, pin optionally freezes to a specific version.

A namespace convention applies to metadata keys, ledger event `kind`, and kinograph entry kinds: bare names are Kinora-reserved; extensions use `prefix::name` (e.g. `kudo::diagram`). Writes are strict (reject unknown bare names); reads are permissive (preserve unknown namespaced names).

### Conflict handling

Ledger file merges are structurally conflict-free by construction (per-lineage files). Version DAG forks are detected at `resolve` time: branch-aware resolution picks the head whose lineage descends from HEAD; otherwise the tool refuses and reports both heads with reconcile options.

Reconciliation is just a regular version event with multiple `parents[]` — no new `kind`. Metadata merges per-field with timestamp-latest wins; events carry only changed fields; `null` removes a field. Timestamps are RFC3339 UTC; no clock-skew handling in MVP.

Out of scope for MVP: CRDT-style per-element merging, logical clocks, interactive reconcile UX.

### CLI packaging

Two-crate workspace: `kinora` (library — store, ledger, kinograph, resolve, render logic) and `kinora-cli` (thin binary wrapper; binary named `kinora` via `[[bin]] name`).

*Why:* Clean library boundary from day one; CLI-only dependencies don't pollute library consumers. Finer-grained splits remain possible later as mechanical, non-breaking refactors.

### Link format

Two link venues by authoring intent:

- **In-content (strong) links** are part of the content hash and parsed by the kind-specific renderer. Kinograph `entries[]` for composition; markdown uses native reference-link syntax with `kino://<id>/` URLs — no Kinora-specific syntax invented.
- **In-metadata (weak) links** live at `metadata.links: [{type, id, name?, pin?, note?}]`; queryable and backlinkable.

The ledger event `kind` names the content type directly. Bare kinds are Kinora core (`markdown`, `text`, `binary`, `kinograph`); extensions are namespaced (`kudo::diagram`, `user::sketch`). MVP implements renderers for `markdown` and `kinograph`; `text`/`binary` are storeable but opaque.

Backlinks are derived (ledger scan + content parse), indexed in the cache layer post-bootstrap.

### Cache path convention

`~/.cache/kinora/<shorthash>-<name>/` where `shorthash` = first 8 hex of `BLAKE3(normalized-repo-url)` and `name` = last path segment of the normalized URL, sanitized to `[a-z0-9_-]`.

The project's `.kinora/config.styx` has a single required field: `repo-url`. URL normalization strips scheme, user-info, `.git`, and trailing slash; converts SSH `:` to `/`; lowercases host (path preserved). `kinora init` auto-fills `repo-url` from `git remote get-url origin` when available.

*Why:* Repo URL is the canonical project identity — multiple clones share cache, different projects with the same basename don't collide. Nix-style `<hash>-<name>` gives a readable-but-unambiguous path.
