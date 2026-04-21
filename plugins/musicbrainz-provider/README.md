# musicbrainz

A STUI metadata provider plugin.

## Build

    stui plugin build

## Test

    stui plugin test

## Install (dev mode)

    stui plugin install --dev

Symlinks the plugin into `~/.stui/plugins/musicbrainz/` for hot-reload.

## Manifest

See `plugin.toml` — declare each verb your plugin implements in `[capabilities.catalog]`.
The required verb is `search = true`. Other verbs are opt-in.
