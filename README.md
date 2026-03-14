# ONYX Server

Self-hosted backend server for the ONYX messenger.

## Features

- WebSocket-based real-time messaging
- Channels and group chats
- File/media sharing
- Ed25519-based authentication
- SQLite database (no external DB required)
- Runs on Linux and Windows

## Building

Requires [Rust](https://rustup.rs/) toolchain.

```bash
cargo build --release
```

## Running

```bash
./onyx-server --config config.toml
```

A default configuration will be generated on first run.

## Client

The desktop/mobile client is available at: https://github.com/wardcore-dev/onyx

## License

Copyright (C) 2026 WARDCORE

Licensed under the **GNU Affero General Public License v3.0 (AGPL-3.0)**.

This means: if you modify and run this software as a network service, you **must** make your modified source code publicly available.

See [LICENSE](LICENSE) for full terms.
