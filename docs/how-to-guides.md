# How-to guides

Task recipes for running and deploying the ConfigComponent. For the full option and topic reference, see
[reference/configuration.md](reference/configuration.md) and
[reference/messaging-interface.md](reference/messaging-interface.md).

## Choose a catalog source

The server loads its catalog through one `catalogSource` descriptor under
`component.global.configComponent`.

Load from a local file and hot-reload on change:

```json
{ "type": "file", "path": "/var/lib/edgecommons/catalog.json", "watch": true }
```

Load from a Kubernetes ConfigMap-mounted file:

```json
{ "type": "configmap", "mountDir": "/etc/edgecommons", "key": "catalog.json", "watch": true }
```

Load from an environment variable — inline JSON, or `@` a file path to keep large content out of the
process environment:

```json
{ "type": "env", "var": "EDGECOMMONS_CONFIG_CATALOG" }
```

Environment sources are snapshot-only; they reject `watch`. Use `file` or `configmap` when you need
hot-reload.

## Serve a component's configuration to a client

A consumer component runs with `-c CONFIG_COMPONENT`, requests its lineage bundle, merges the ordered
`layers[].config`, and validates the merged effective config itself. The server never merges layers — it
returns the lineage in order. Key each catalog `components` entry by the consumer's short component
lookup token (the segment after the last dot of its full Greengrass name), and parent it at the deepest
shared scope node it should inherit from.

## Enable volatile catalog updates for testing

Turn on the `update-catalog` message interface only in non-production:

```json
{ "component": { "global": { "configComponent": { "allowVolatileCatalogUpdates": true } } } }
```

Then send a complete replacement whose top-level `version` matches `catalog.version`. The server promotes
it to the active in-memory cache and pushes bundles, but never writes it back to the source — it is gone
on restart or on the next source-side reload. Keep this `false` in production and deny publish access to
the update topic where the broker or IPC policy allows.

## Deploy on Greengrass

`build.sh` builds the Linux binary with the `greengrass` feature and stages the artifact and recipe:

```bash
gdk component build
```

The recipe defaults the catalog source to a file under the component's work directory and grants only
the rendezvous topics the server needs. The `greengrass` feature is Linux-only because the Greengrass
IPC SDK is Linux-only.

## Deploy on Kubernetes

`k8s/` ships a `ConfigMap` and a `Deployment`. The ConfigMap carries both the bootstrap `config.json`
(its `catalogSource` is a `configmap` source pointing at the sibling `catalog.json` key) and the
`catalog.json` the server hands out. The Deployment mounts the ConfigMap as a whole directory at
`/etc/edgecommons` so the config source and catalog source both hot-reload on a `kubectl apply`:

```bash
# build and load the image (or push to your registry and set image: in k8s/deployment.yaml)
docker build -t config-component:latest .
kind load docker-image config-component:latest
kubectl apply -f k8s/
```

Edit either ConfigMap key and re-apply to drive a hot reload with no restart.

## Build against a local core checkout

`Cargo.toml` pins the `edgecommons` library by git rev. For local development against a sibling
`core/libs/rust` checkout, add a gitignored `.cargo/config.toml`:

```toml
[patch."https://github.com/edgecommons/edgecommons.git"]
edgecommons = { path = "../core/libs/rust" }

[net]
git-fetch-with-cli = true
```

CI never sees that file, so it resolves the committed pin. To confirm the standalone path builds, move
`.cargo` aside and run `cargo build --locked && cargo test --locked`.
