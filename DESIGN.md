# DESIGN — config-component

> Treat this document as the **design-fidelity contract** for this component: before changing
> behavior, update the relevant section here in the same change, and review new work against what is
> written here — not against a summary of it. Status, history, and roadmap live here (internal);
> the public `docs/` describe only current behavior.

## What it is

`com.mbreissi.edgecommons.ConfigComponent` is the dedicated **service** that backs `CONFIG_COMPONENT`
hierarchical-config deployments. Ordinary EdgeCommons components select `-c CONFIG_COMPONENT` and remain
clients: they request their lineage bundle from this server, merge `layers[].config` locally, validate
the merged effective config, and hot-reload when the server pushes a replacement bundle. This server
loads a hierarchy catalog from a `CatalogSource`, serves ordered lineage bundles, and pushes accepted
catalog updates to consumers over the configured messaging transport.

It is a service, so only the cross-cutting component baseline applies — there is deliberately no
southbound `sb/*` command family, no `southbound_health` metric, no edge-console panel trio, and no
device seam (see D-CC-8).

## Decisions

- **D-CC-1. Core dependency shape: pinned git rev + gitignored local `[patch]` override.** `Cargo.toml`
  pins `edgecommons` by git `rev` (the current core `main` rev the sibling components share). Local dev
  overrides it to the sibling `../core/libs/rust` checkout via a gitignored `.cargo/config.toml`
  `[patch]` block (`/.cargo/` is gitignored). This replaces the previous committed
  `path = "../core/libs/rust"` primary dependency, which only resolved from the umbrella working tree
  and left a standalone clone and hosted CI unable to build. `Cargo.lock` is committed and records the
  git source (regenerated with the patch disabled). The `Dockerfile` builds from the repo root against
  the pinned dep — the umbrella-context `COPY core/...` workaround is gone. *(baseline item 2)*

- **D-CC-2. CONFIG_COMPONENT rendezvous is served manually, not through a library facade.** The server
  subscribes to the two reserved rendezvous topics and answers them itself:
  `ecv1/{device}/config/cmd/get-configuration` (returns a lineage bundle) and
  `ecv1/{device}/config/cmd/update-catalog` (volatile in-memory replacement). Pushes go to each
  component's own `ecv1/{device}/{component}/cmd/set-config`. Bootstrapping from `CONFIG_COMPONENT`
  itself is rejected before any subscription is created, so the server can never depend on itself.

- **D-CC-3. The server does not merge layers.** A `get-configuration` reply is an ordered `layers[]`
  bundle from the highest shared scope down to the requested component; each layer is a raw partial
  config. Clients merge and validate their own effective config. The server validates catalog structure,
  lineage parentage, scope monotonicity, and identity ownership — but never schema-validates a raw layer
  as an effective component config.

- **D-CC-4. Volatile message updates are off by default and never persist.** `update-catalog` is a
  non-production debug/verification/test interface, gated by
  `component.global.configComponent.allowVolatileCatalogUpdates` (default false). An accepted update is
  promoted only to the active in-memory cache and, when `pushOnCatalogReload` is true, pushed to every
  component; it is never written back to the file/ConfigMap source and does not survive a restart or a
  later source-side reload. Disabled, invalid, or malformed updates keep the previous active catalog and
  do not push (reject-and-keep).

- **D-CC-5. Catalog sources are a pluggable seam.** `CatalogSource` (`src/source.rs`) abstracts loading;
  v1 ships file, Kubernetes ConfigMap (read/watch only — the server never writes back to a ConfigMap),
  and environment-variable sources, plus a read-only snapshot source for tests. File and ConfigMap
  sources may watch (content-hash-polled) and hot-promote; environment sources are snapshot-only.

- **D-CC-6. D-U28 topic shape.** Request/reply and push topics follow the canonical UNS grammar
  `ecv1/{device}/{component}/{instance}/{class}[/channel]` with the reserved `config` component token
  for the rendezvous inbox and each consumer's own `{component}/cmd/set-config` for pushes. Device and
  component tokens are sanitized with the shared UNS blacklist.

- **D-CC-7. Shared conformance vectors are vendored.** `tests/catalog_vectors.rs` runs the shared
  cross-language hierarchical-config conformance vectors. Their canonical home is the core repo at
  `core/hierarchical-config-test-vectors/catalogs.json`; that path is only reachable from the umbrella,
  so a standalone clone and single-repo CI could not run the suite. The vectors are vendored into
  `tests/vectors/catalogs.json` (a verbatim copy). When the canonical vectors change, re-copy the file —
  do not edit the vendored copy in place. This keeps the repo genuinely standalone-testable (the intent
  of D-CC-1) without forking the conformance contract.

- **D-CC-8. License is BUSL-1.1.** `Cargo.toml` `license`, the README, and `LICENSE` all state BUSL-1.1;
  the earlier `Cargo.toml license = "Apache-2.0"` metadata mismatch (tracked as #2) is fixed. *(closes #2)*

## Config

`config.schema.json` is the source of truth for `component.global` — the object at
`component.global.configComponent`: a `catalogSource` descriptor (one of the file / configmap / env
shapes) plus the `pushOnCatalogReload` and `allowVolatileCatalogUpdates` booleans. The component reads
no per-instance config (`component.instances[]` is always empty). The library envelope is owned by the
canonical schema and is not redeclared here.

## Command / message surface

Two served rendezvous verbs (`get-configuration`, `update-catalog`) and one push (`set-config`), all on
the normal EdgeCommons envelope. Error codes: `BAD_REQUEST`, `CONFIG_NOT_FOUND`, `CATALOG_UNAVAILABLE`,
`CATALOG_INVALID`, `CATALOG_UPDATE_DISABLED`, and the lineage codes (`LINEAGE_CYCLE`,
`LINEAGE_PARENT_MISSING`, `LINEAGE_DEPTH_EXCEEDED`, `LINEAGE_SCOPE_CONFLICT`, `LINEAGE_IDENTITY_CONFLICT`).
See `docs/reference/messaging-interface.md`.

## Metrics

The component emits no custom application metrics. It runs the standard library envelope (heartbeat →
`state`, and whatever `metricEmission` target is configured); see `docs/reference/metrics.md`.

## Kubernetes deployment materials

The registry entry and docs list KUBERNETES as a supported platform via the `configmap` catalog source.
`k8s/` ships a `ConfigMap` (the bootstrap config, including a `configmap`-type `catalogSource` and a
second ConfigMap key carrying the catalog itself) and a `Deployment` (non-root, read-only rootfs, the
`/etc/edgecommons` config volume mounted whole for hot-reload, health probes). Resource names use the
kebab binary name per baseline item 1. This resolves the issue's P2 observation ("KUBERNETES is a
claimed platform with no k8s manifests") by adding the manifests rather than recording an N/A.

## Validation

- `cargo test` — catalog parsing/lineage, the catalog sources + descriptor factory, and the
  coordinator's promote/serve/update/reload logic, no broker required.
- `cargo clippy --all-targets -- -D warnings`.
- `cargo llvm-cov --fail-under-lines 90 --ignore-filename-regex '(server|main)\.rs'` — the coverage
  gate. Only the messaging runtime seam is excluded (see AGENTS.md / the `ci.yml` comment); all pure
  logic stays in the denominator.
- Fresh-clone build proof — `cargo build --locked` + `cargo test --locked` with `.cargo` moved aside —
  resolves the pinned git dependency the way a fresh clone / CI does.
- Greengrass IPC feature build is Linux-only (`build.sh --features greengrass`), validated on WSL/lab.
