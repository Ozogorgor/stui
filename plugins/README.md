# Bundled plugins

This directory ships the metadata plugins that form stui's reference
plugin set. Non-metadata plugins (subtitles, torrents, streams) live in
the separate [`stui_plugins`](https://github.com/Ozogorgor/stui_plugins)
repository.

Every bundled plugin implements `stui_plugin_sdk::Plugin` +
`CatalogPlugin`, exports the WASM ABI via
`stui_export_catalog_plugin!(<name>)`, and declares its manifest in
`plugin.toml` per the canonical schema
(`[plugin] / [meta] / [env] / [[config]] / [permissions] /
[permissions.rate_limit] / [capabilities.catalog]`).

## End-state matrix

| Plugin        | Kinds              | Verbs                                             | API key                  | Upstream           |
|---------------|--------------------|---------------------------------------------------|--------------------------|--------------------|
| `tmdb`        | movie, series, ep. | search, lookup, enrich, artwork, credits, related | required (`TMDB_API_KEY`)| api.themoviedb.org |
| `omdb`        | movie, series      | search, lookup                                    | required (`OMDB_API_KEY`)| omdbapi.com        |
| `anilist`     | movie, series      | search, lookup, enrich, artwork, credits, related | none                     | graphql.anilist.co |
| `kitsu`       | movie, series      | search, lookup                                    | optional                 | kitsu.io           |
| `discogs`     | artist, album      | search, lookup                                    | optional (unauth = slower)| api.discogs.com   |
| `lastfm`      | artist, album, trk | search, enrich                                    | required (`LASTFM_API_KEY`)| libre.fm         |
| `musicbrainz` | artist, album, trk | search, lookup, enrich, artwork, credits, stub-related | none               | musicbrainz.org    |

## Building

### A single plugin

```sh
cd plugins/<name>
cargo run -p stui -- plugin build        # compiles to target/wasm32-wasip1/debug/
cargo run -p stui -- plugin lint         # validates plugin.toml against the schema
cargo run -p stui -- plugin test         # runs its unit tests
```

### Whole plugin workspace

```sh
cd plugins
cargo build --target wasm32-wasip1 --workspace   # 7 plugins build
```

## Dev-installing

`stui plugin install --dev` symlinks the plugin directory into
`~/.stui/plugins/<name>/`. The dev-install expects `plugin.wasm` to sit
next to `plugin.toml`; after `cargo build --target wasm32-wasip1 -p <plugin>`
you can either copy or symlink the artifact in:

```sh
ln -s $(cargo metadata --format-version 1 | jq -r '.target_directory')/wasm32-wasip1/debug/<crate_name>.wasm plugin.wasm
```

The `plugin.wasm` path is gitignored per the `.gitignore` rule.

## API keys

Plugins that require a key read it from `InitContext.config["api_key"]`
first, then fall back to the corresponding env var
(`TMDB_API_KEY`, `OMDB_API_KEY`, …). When running the runtime daemon
directly (not via the TUI), set them through `~/.stui/secrets.env`:

```sh
cat <<'EOF' > ~/.stui/secrets.env
TMDB_API_KEY=<your-tmdb-key>
OMDB_API_KEY=<your-omdb-key>
LASTFM_API_KEY=<your-lastfm-key>
EOF
chmod 600 ~/.stui/secrets.env
```

Then launch the daemon with the file sourced:

```sh
set -a; . ~/.stui/secrets.env; set +a
stui-runtime daemon
```

Plugins without required keys (`anilist`, `kitsu`, `musicbrainz`,
`discogs`) load straight to `Loaded`; plugins needing a key load to
`NeedsConfig { missing: ["api_key"] }` until the key is provided, after
which a reload transitions them to `Loaded`.

## External vs bundled

Bundled plugins are allowed to ship manifest stubs (e.g. `related =
{ stub = true, reason = "…" }` on `musicbrainz`) because they're
installed from the in-tree workspace — `stui plugin lint` warns but
passes. External plugins prepping for the future Tier-3 registry will
use `stui plugin build --release`, which (once the gate lands — see
`docs/superpowers/BACKLOG.md`) rejects declared stubs. Either way,
unimplemented verbs fall through to the trait's `NOT_IMPLEMENTED`
default without the plugin needing explicit body code.

## Adding a new plugin

```sh
cd plugins
cargo run -p stui -- plugin init <name>
# then edit plugin.toml + src/lib.rs per the scaffolded template
```

The template wires `Plugin::manifest` + `Plugin::init` + stub
`CatalogPlugin::search`; flesh out whichever verbs your upstream
supports, update the manifest's `[capabilities.catalog]` block
accordingly, and add the crate to `plugins/Cargo.toml` workspace
members.
