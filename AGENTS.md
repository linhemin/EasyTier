# Repository Guidelines

## Project Structure & Module Organization
- `easytier/`: Core Rust crate with binaries `easytier-core` and `easytier-cli` (source in `easytier/src`). Tests live in `easytier/src/tests`.
- `easytier-web/`: Rust web service with bundled frontend (`frontend`, `frontend-lib`).
- `easytier-gui/`: Tauri desktop app (`src` for Vue UI, `src-tauri` for Rust). 
- `tauri-plugin-vpnservice/`: Tauri plugin used by the GUI.
- `.github/workflows/`: CI pipelines. `assets/`, `script/`: shared assets and install scripts.

## Build, Test, and Development Commands
- Build core: `cargo build --release` (or target-specific via `--target ...`).
- Build workspace: `cargo build -p easytier -p easytier-web` (default members).
- Build GUI frontend/libs: `pnpm -r build` (from repo root; requires pnpm v9+).
- Build GUI app: `cd easytier-gui && pnpm tauri build --target <triple>` (e.g., `x86_64-apple-darwin`).
- Run tests (core): `cargo test --no-default-features --features=full --verbose`.
- Dev GUI: `cd easytier-gui && pnpm dev`.

## Coding Style & Naming Conventions
- Rust edition 2021; 4-space indent; prefer explicit types at public boundaries.
- Run format/lints before commit: `cargo fmt --all`, `cargo clippy --all-targets --all-features -D warnings`.
- Naming: modules/files `snake_case`, types/traits `CamelCase`, constants `SCREAMING_SNAKE_CASE`, functions `snake_case`.
- Logging via `tracing`; errors via `thiserror`/`anyhow` as used in core.
- Frontend (GUI): TypeScript + Vue; lint with `pnpm --filter easytier-gui lint` or `lint:fix`.

## Testing Guidelines
- Unit/integration tests use Rust’s test framework plus `serial_test`/`rstest` where needed.
- Some Linux integration tests require network namespaces and bridge tools (`ip`, `brctl`) and may need root. Prefer running unit tests locally; use CI for full integration where possible.
- Add tests for new features and bug fixes; keep tests isolated and deterministic.

## Commit & Pull Request Guidelines
- Branch from `develop` (e.g., `feature/<name>`, `fix/<name>`). 
- Conventional commit style preferred: `feat: ...`, `fix: ...`, `refactor: ...`, `docs: ...`.
- Before pushing: build, format, and lint Rust and GUI.
- PRs: target `develop`, include a clear description, linked issues (`Closes #123`), and screenshots for UI changes. Update docs when behavior changes.

## Notes & Tips
- Feature flags: core defaults include `wireguard`, `websocket`, `smoltcp`, `tun`, `socks5`, `quic`; use `--features full` when tests or builds require it.
- Cross-compilation toolchains are preconfigured in `.cargo/config.toml`; ensure required linkers/SDKs are installed for your target.
