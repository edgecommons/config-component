# config-component (Claude Code)

EdgeCommons service component (Rust), `com.mbreissi.edgecommons.ConfigComponent`. The full picture —
what this component is, the `CatalogSource` seam, config location, and the org conventions it inherits
— lives in `AGENTS.md` and is shared with every agent tool. It is imported here in full:

@AGENTS.md

## Local-dev notes

- The `edgecommons` dependency in `Cargo.toml` is a git **`rev`** pin (the current core `main` rev the
  sibling components use). To build against your local sibling checkout instead of fetching the pin,
  the gitignored `.cargo/config.toml` carries a `[patch."https://github.com/edgecommons/edgecommons.git"]`
  block pointing `edgecommons` at `../core/libs/rust` (adjust the absolute path for your machine). CI
  never sees that file, so it always resolves the committed pin.
- `Cargo.lock` is committed and records the **git** source (regenerated with the `[patch]` disabled).
  When the local patch is active, a plain `cargo build` may rewrite the lock to the path source — do
  not commit that churn.
- Prove the standalone path before pushing: move `.cargo` aside and run
  `CARGO_NET_GIT_FETCH_WITH_CLI=true cargo build --locked && cargo test --locked`. That resolves the
  pinned git dependency the way a fresh clone and CI do.
- On Windows the default `standalone` feature builds and tests natively. The `greengrass` feature is
  Linux-only (the Greengrass IPC SDK is Linux-only); `build.sh` opts into it for GDK packaging.
