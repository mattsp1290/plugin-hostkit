# Publication audit

## H1 scaffold (2026-10-03)

No source imported. Package name and placeholder README do not use the format trademark.
MIT copyright holder provisionally Matt Spurlin, pending owner confirmation before import publication.

L5: `cargo package --list --allow-dirty` lists Cargo manifest/lockfile, LICENSE, README, and src/lib.rs only (plus Cargo-generated metadata).
`cargo tree -e normal` contains no application dependencies. `cargo license --json` reports permissive alternatives for every dependency: MIT, Apache-2.0, BSD-3-Clause, ISC, Unicode-3.0 or Zlib (no copyleft requirement).
No local paths, application identifiers, or credential values exist in scaffold files.

Local gates: cargo build --all-targets, cargo test, cargo clippy --all-targets -- -D warnings passed. Scaffold has zero tests by design. Remote two-platform CI evidence pending push.

## Imported code

Not yet approved for publication. L1–L5 evidence and explicit owner approval required before first imported-code push.
