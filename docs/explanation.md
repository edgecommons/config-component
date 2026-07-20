# Explanation — Why the ConfigComponent works the way it does

## A dedicated server, not a library facade

`CONFIG_COMPONENT` is a rendezvous: any component can select it as its config source and ask "what is my
configuration?" over the bus. The ConfigComponent is the one process that answers. It bootstraps from an
ordinary source (`GG_CONFIG`, `FILE`, `ENV`, or `CONFIGMAP`) — never from `CONFIG_COMPONENT` itself, which
is rejected before any subscription is created — so the server can never depend on the very service it
provides. Keeping this a separate deployable component, rather than a mode of the client library, means a
site runs exactly one authority for hierarchical config and every other component stays a thin client.

## The server serves layers; clients merge

A `get-configuration` reply is an ordered list of `layers[]`, from the highest shared scope down to the
requested component. The server does **not** merge them. This is deliberate: merge semantics (deep merge,
per-key override, array handling) belong to the client library that also validates the merged result
against the component's own schema. If the server merged, it would have to know every component's schema
and merge rules, and a merge bug would corrupt config for every consumer at once. Serving raw ordered
layers keeps the server's job narrow — parse, validate structure, order the lineage — and lets each client
own the last mile.

What the server *does* validate is structural: the hierarchy ends in `device`, node ids match their own
scope claim, scopes grow monotonically down a lineage, parents exist, there are no cycles, and no layer's
`identity` conflicts with an ancestor's scope. These are properties no single client could check, because
they span the whole catalog.

## Reject-and-keep

Whenever the server considers a new catalog — a source-side reload or a volatile message update — it either
promotes a fully valid catalog or keeps the one it already has. There is no partial promotion. A reload
that fails to parse, or a disabled/invalid update, leaves the active catalog untouched and pushes nothing.
A component that is running on a good catalog is never knocked onto a broken one by a bad edit upstream.

## Volatile updates are a test affordance, not a control plane

The `update-catalog` message interface exists for debug, verification, and test environments. It is off by
default, and even when on it only ever touches the in-memory cache: the replacement is never written back
to the file or ConfigMap source, so it vanishes on restart or on the next source-side reload. The durable
source of truth is always the catalog file or ConfigMap. This keeps a convenient test hook from quietly
becoming an unaudited way to mutate production config.

## Hot reload without restarts

File and ConfigMap sources poll for content changes by hash and hot-promote a new catalog in place, then
push fresh `set-config` bundles to every component. On Kubernetes the ConfigMap is mounted as a whole
directory so the kubelet's atomic `..data` swap is observed as a single content change. This is why the
Deployment mounts the config volume whole and never via `subPath` — a `subPath` mount would not see the
swap, and hot reload would silently stop working.
