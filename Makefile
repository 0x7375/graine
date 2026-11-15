.PHONY: devshell init-fixtures backend-dev fixture-run frontend-dev build check

devshell:
	nix develop

init-fixtures:
	./scripts/init-fixtures.sh

# Dashboard-only backend: management_enabled=false in config.dev.toml.
backend-dev:
	watchexec --restart --watch src --watch Cargo.toml --watch config.dev.toml -- cargo run -- --config config.dev.toml

# Run one read-only fixture scan to populate current ranking and disk calculations.
fixture-run: init-fixtures
	cargo run -- --config config.dev.toml --once --dry

frontend-dev:
	cd web && pnpm dev

build:
	cd web && pnpm build
	cargo build --release

check:
	cargo fmt --check
	cargo check
	cargo test
	cargo clippy --all
	cd web && pnpm typecheck
