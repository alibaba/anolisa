# AgentSecCore V2 Policy Engine Framework (asc-policy) — Build System
#
# Self-contained targets for the v2 Rust workspace. Builds enforce the
# committed Cargo.lock (--locked), matching the repo convention
# (agent-memory, anolisa). Integration into the component-level Makefile
# (src/agent-sec-core/Makefile) is deferred to the first P0 feature PR.

BUILD_DIR ?= target

.PHONY: help
help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

.PHONY: build
build: ## Build all crates (release, locked)
	cargo build --release --workspace --locked

.PHONY: fmt
fmt: ## Format all crates
	cargo fmt --all

.PHONY: fmt-check
fmt-check: ## Check formatting without modifying
	cargo fmt --all -- --check

.PHONY: lint
lint: ## Run clippy with warnings denied
	cargo clippy --workspace --all-targets --locked -- -D warnings

.PHONY: test
test: ## Run all workspace tests (locked)
	cargo test --workspace --locked

.PHONY: doc
doc: ## Build API docs (no deps)
	cargo doc --workspace --no-deps --locked

.PHONY: check
check: fmt-check lint test doc ## Run the full pre-commit gate

.PHONY: clean
clean: ## Remove build artifacts
	cargo clean
