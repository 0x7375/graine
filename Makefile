.PHONY: devshell init-fixtures fixture-check backend-dev fixture-backend-dev fixture-run frontend-dev build check

devshell:
	nix develop

init-fixtures:
	./scripts/init-fixtures.sh

fixture-check:
	python3 scripts/check-fixtures.py

# Dashboard-only backend: management_enabled=false in config.dev.toml.
backend-dev:
	watchexec --restart --watch src --watch Cargo.toml --watch config.dev.toml -- cargo run -- --config config.dev.toml

# Fresh synthetic dataset and history, entirely separate from data/dev.sqlite3.
fixture-run: init-fixtures fixture-check
	cargo run -- --config config.fixture.toml --once --dry

# Serve the synthetic dashboard for UI development (terminal 1).
fixture-backend-dev:
	watchexec --restart --watch src --watch Cargo.toml --watch config.fixture.toml -- cargo run -- --config config.fixture.toml

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
