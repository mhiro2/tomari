.DEFAULT_GOAL := help

.PHONY: help
help: ## Show available make targets.
	@awk 'BEGIN { FS = ":.*##" } \
		/^## .* ##$$/ { \
			title = $$0; \
			gsub(/^## /, "", title); \
			gsub(/ ##$$/, "", title); \
			printf "\n\033[1m%s\033[0m\n", title; \
			next; \
		} \
		/^[a-zA-Z0-9][a-zA-Z0-9_-]+:.*##/ { \
			desc = $$2; \
			gsub(/^[ \t]+/, "", desc); \
			printf "  \033[36m%-24s\033[0m %s\n", $$1, desc; \
		}' $(MAKEFILE_LIST)

## Setup ##

.PHONY: setup
setup: deps hooks ## Install dependencies and Git hooks.

.PHONY: deps
deps: ## Install frontend dependencies from the lockfile.
	pnpm install --frozen-lockfile

.PHONY: hooks
hooks: ## Install lefthook Git hooks.
	lefthook install

.PHONY: setup-tools
setup-tools: ## Install Rust development tools.
	cargo install cargo-deny --locked
	cargo install cargo-nextest --locked

## Format ##

.PHONY: fmt
fmt: rust-fmt frontend-fmt ## Format all Rust and frontend code.

.PHONY: fmt-check
fmt-check: rust-fmt-check frontend-fmt-check ## Check formatting without modifying files.

.PHONY: rust-fmt
rust-fmt: ## Format Rust code.
	cargo fmt --all

.PHONY: rust-fmt-check
rust-fmt-check: ## Check Rust formatting.
	cargo fmt --all -- --check

.PHONY: frontend-fmt
frontend-fmt: ## Format frontend code with oxfmt.
	pnpm fmt

.PHONY: frontend-fmt-check
frontend-fmt-check: ## Check frontend formatting with oxfmt.
	pnpm fmt:check

## Lint ##

.PHONY: lint
lint: rust-lint frontend-lint ## Lint all Rust and frontend code.

.PHONY: rust-lint
rust-lint: ## Run clippy across the workspace with warnings treated as errors.
	cargo clippy --workspace --all-targets -- -D warnings

.PHONY: frontend-lint
frontend-lint: ## Lint frontend code with oxlint.
	pnpm lint

.PHONY: typecheck
typecheck: ## Type-check frontend code with tsc.
	pnpm typecheck

## Test ##

.PHONY: test
test: rust-test frontend-test ## Run all Rust and frontend tests.

.PHONY: rust-test
rust-test: ## Run Rust workspace tests.
	cargo nextest run --workspace

.PHONY: frontend-test
frontend-test: ## Run frontend tests with Vitest.
	pnpm test

## Build & Run ##

.PHONY: build
build: rust-build frontend-build ## Build the Rust workspace and frontend.

.PHONY: rust-build
rust-build: ## Build the Rust workspace in debug mode.
	cargo build --workspace

.PHONY: frontend-build
frontend-build: ## Build the production frontend.
	pnpm build

.PHONY: bundle
bundle: ## Build the macOS application bundles with Tauri.
	pnpm tauri build

.PHONY: dev
dev: ## Run the Tauri application with frontend hot reload.
	pnpm tauri dev

.PHONY: clean
clean: ## Remove Rust and frontend build artifacts.
	cargo clean
	rm -rf dist

## Audit ##

.PHONY: deny-check
deny-check: ## Audit Rust advisories, licenses, bans, and sources.
	cargo deny check all

.PHONY: check
check: fmt-check lint typecheck test deny-check frontend-build ## Run the complete local CI suite.
