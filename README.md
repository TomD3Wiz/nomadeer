# Nomadeer

A private native desktop app for tracking EVE Online wormhole chains while exploring down-chain.

This scaffold is a Rust GUI application built with `egui`/`eframe`. It stores your local chain data in SQLite at `data/nomadeer.sqlite3` and does not run a web server.

## Current features

- Native desktop window.
- Track systems, signatures, wormhole connections, visits, and notes.
- Track local accounts as the foundation for ESI authorization.
- Visual chain map with mass/status coloring.
- Local SQLite persistence.
- Single binary target via Cargo.

## Build

```sh
cargo build
```

Or use the Makefile:

```sh
make build
make run
make qa
```

For an optimized binary:

```sh
cargo build --release
```

## Run

```sh
cargo run
```

Release binary path:

```text
target/release/nomadeer
```

## Data

The app writes local private data to:

```text
data/nomadeer.sqlite3
```

Those files are ignored by git. Existing `data/eve_wanderer.sqlite3` data is copied to the new Nomadeer database path on first launch. Existing `data/eve_wanderer.json` data is imported into SQLite on first empty database load.

## Roadmap ideas

- Editable records instead of delete/recreate.
- Drag/drop chain map nodes and saved layout positions.
- Wormhole type presets for mass, life, and ship size.
- ESI integration for current character location.
- Import/export chain snapshots.
- Optional encrypted data store.

## Developer tooling

Common commands:

```sh
make fmt        # format code
make clippy     # lint with warnings denied
make test       # run tests
make audit      # check dependency advisories
make deny       # check advisories/licenses/bans/sources
make coverage   # run coverage via cargo-llvm-cov
make qa         # fmt-check + clippy + test + audit + deny
```

The project pins the Rust toolchain through `rust-toolchain.toml`.
