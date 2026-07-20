# config-component — component notes

EdgeCommons **service component** (Rust). Full name `com.mbreissi.edgecommons.ConfigComponent`,
crate/binary `config-component`. It is the dedicated server for `CONFIG_COMPONENT` hierarchical-config
deployments: it loads a hierarchy catalog, serves ordered lineage bundles to EdgeCommons components,
and pushes accepted catalog updates to consumers over the configured EdgeCommons messaging transport.
Depends on the `edgecommons` Rust library. If this repo lives inside the EdgeCommons org umbrella
workspace, read its root `AGENTS.md` first (org repo map, design-fidelity contract, validation matrix,
platform/transport model); everything below is this component's own detail.

## What it is

A **service**, not a southbound adapter: there is no device connection, no `sb/*` command family, no
`southbound_health` metric, no edge-console panel trio, and no device seam. It runs on `GREENGRASS` /
`HOST` / `KUBERNETES` via `edgecommons` — no platform branching in this component's own code. The
component must bootstrap from a non-`CONFIG_COMPONENT` source (`GG_CONFIG`, `FILE`, `ENV`, or
`CONFIGMAP`); launching it with `-c CONFIG_COMPONENT` is rejected before any subscription is created.

## The seam

`src/source.rs` defines the `CatalogSource` trait — the one abstraction the rest of the crate is built
against, so the v1 mounted-file/env sources can be replaced without touching request handling:

- `FileCatalogSource` — a local JSON file, with optional content-hash-polled watch/hot-reload.
- `ConfigMapCatalogSource` — a Kubernetes ConfigMap-mounted file (read/watch only; never writes back).
- `EnvCatalogSource` — inline JSON or `@/path` in an environment variable (snapshot only).
- `ReadOnlyCatalogSource` — a fixed snapshot, for tests and future read-only backends.

`source_from_descriptor` maps a `catalogSource` config descriptor to one of these. `src/catalog.rs`
parses and validates a catalog and builds lineage bundles; `src/coordinator.rs` owns the active
snapshot and the promote / serve / reject-and-keep update semantics; `src/server.rs` is the messaging
runtime that binds them to the `CONFIG_COMPONENT` rendezvous topics.

## Config location

This component's own settings live under `component.global.configComponent` in the EdgeCommons config
document (`config.schema.json` is the contract): the `catalogSource` descriptor plus
`pushOnCatalogReload` and `allowVolatileCatalogUpdates`. It reads no per-instance config —
`component.instances[]` is always empty. The sibling sections (`logging`, `messaging`,
`metricEmission`, `heartbeat`, `hierarchy`, `identity`) are the standard `edgecommons` envelope, owned
by the canonical schema and not redeclared here. `test-configs/` carries a runnable example plus a
sample catalog.

## Validation expectations

- `cargo test` covers the pure logic (catalog parsing/lineage, the catalog sources + descriptor
  factory, the coordinator's promote/serve/update/reload) directly — no broker required. The
  `catalog_vectors.rs` suite runs the shared cross-language conformance vectors, vendored into
  `tests/vectors/catalogs.json` so the suite runs from a standalone clone (see DESIGN §D-CC-7).
- `cargo llvm-cov --fail-under-lines 90` is the coverage gate (`.github/workflows/ci.yml`'s `coverage`
  job) — the org rule is 90% line coverage per language. The coverage job passes
  `--ignore-filename-regex '(server|main)\.rs'` to exclude ONLY the messaging runtime seam
  (`server.rs`'s subscribe/publish/reply wiring + `main.rs`'s bootstrap shim), which needs a live
  `EdgeCommons` runtime + broker and is exercised by the scaffold→build gate and HOST/Greengrass smoke.
  All pure logic stays IN the denominator. Do not lower the gate or exclude testable code to pass it —
  add tests.
- `edgecommons component validate` checks this repo's config against `config.schema.json` and warns if
  `Cargo.lock` is not committed (it is committed here, git-sourced).
- A **fresh-clone build proof** — `cargo build --locked` + `cargo test --locked` with the local
  `.cargo` `[patch]` override disabled — must pass: the repo builds and tests standalone against the
  pinned git dependency (the point of the dependency-shape flip, DESIGN §D-CC-1).

## Org conventions this component inherits

- Builders/facades are the construction path (`MessageBuilder`, the messaging facades) — never
  hand-built envelopes.
- Reference docs describe the current contract only — no roadmap/status/migration narration in
  `docs/`; that lives here and in `DESIGN.md`.
- Runtime artifacts (logs, build output, local broker state, Greengrass build staging) stay out of Git.
- The core dependency is pinned by git rev in `Cargo.toml`; local dev overrides it to the sibling
  checkout through a gitignored `.cargo/config.toml` `[patch]` block (see `CLAUDE.md`).
