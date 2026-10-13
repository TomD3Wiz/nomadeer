# Nomadeer

A private native desktop app for tracking EVE Online wormhole chains while exploring down-chain.

This scaffold is a Rust GUI application built with `egui`/`eframe`. It stores your local chain data in SQLite at `data/nomadeer.sqlite3` and does not run a web server.

## Current features

- Native desktop window.
- Track systems, signatures, wormhole connections, visits, and notes.
- Authorize characters with EVE ESI using local loopback OAuth + PKCE.
- Sync authorized character location from ESI and infer movement connections.
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

## ESI setup

Create an application at <https://developers.eveonline.com/> and use this callback URL:

```text
http://localhost:8788/callback
```

In Nomadeer, enter the app's Client ID under **ESI settings**. The default scopes are:

```text
publicData esi-location.read_location.v1 esi-location.read_ship_type.v1 esi-search.search_structures.v1 esi-universe.read_structures.v1 esi-ui.open_window.v1 esi-ui.write_waypoint.v1 esi-location.read_online.v1 esi-planets.read_customs_offices.v1 esi-structures.read_corporation.v1 esi-structures.read_character.v1 esi-activities.read_character.v1 esi.activity.char:read esi.cosmetic.char:read
```

Then add an account placeholder, authorize it from the **Accounts** tab, and use **Sync location** to pull the character's current system. Access and refresh tokens are stored only in the local SQLite database.

## Roadmap ideas

- Editable records instead of delete/recreate.
- Drag/drop chain map nodes and saved layout positions.
- Wormhole type presets for mass, life, and ship size.
- Background ESI location polling.
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
