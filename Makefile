# Lanweave development tasks.
#
# Targets mirror the CI workflow at .github/workflows/ci.yml. Run `make ci`
# before pushing to replicate the full gate locally. `cargo --locked` is used
# to match CI and prevent unintentional dependency drift.

BIN        := lanweave
CARGO      := cargo
CARGO_AUDIT_VERSION := ^0.22
CARGO_DENY_VERSION  := ^0.20

.DEFAULT_GOAL := help

.PHONY: help build run test fmt fmt-check clippy clippy-strict audit deny ci-tools check ci doc clean

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; printf "Lanweave targets:\n"} \
	    /^[a-zA-Z_-]+:.*## / {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}' \
	    $(MAKEFILE_LIST)

build: ## Build the debug binary
	$(CARGO) build --locked

run: ## Run the debug binary
	$(CARGO) run --locked

test: ## Run the test suite
	$(CARGO) test --all --locked

fmt: ## Apply cargo fmt
	$(CARGO) fmt --all

fmt-check: ## Verify cargo fmt (no changes)
	$(CARGO) fmt --all -- --check

clippy: ## Run clippy (advisory)
	$(CARGO) clippy --all-targets --locked

clippy-strict: ## Run clippy with warnings as errors
	$(CARGO) clippy --all-targets --locked -- -D warnings

audit: ## Run cargo audit (security advisories)
	$(CARGO) audit

deny: ## Run cargo deny (licenses, sources, bans)
	$(CARGO) deny check

ci-tools: ## Install CI-pinned cargo-audit and cargo-deny
	cargo install --locked cargo-audit --version "$(CARGO_AUDIT_VERSION)"
	cargo install --locked cargo-deny  --version "$(CARGO_DENY_VERSION)"

check: fmt-check clippy-strict test ## Pre-push local gate (no audit/deny)

ci: ci-tools fmt-check clippy-strict test audit deny ## Full CI gate, locally

doc: ## Build documentation
	$(CARGO) doc --all --locked --no-deps

clean: ## Remove build artifacts
	$(CARGO) clean
