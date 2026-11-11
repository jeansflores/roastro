.DEFAULT_GOAL := help

SPEED ?= 10

.PHONY: help build test lint fmt run-sim monitor probe

help: ## Show this help
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  %-10s %s\n", $$1, $$2}'

build: ## Build the release binaries into ./target/release
	cargo build --workspace --release

test: ## Run all tests
	cargo test --workspace

lint: ## Check formatting and run clippy with warnings as errors
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format code
	cargo fmt --all

run-sim: ## Run the simulator with the autopilot (SPEED=10)
	cargo run -q -p sim -- --auto --speed $(SPEED)

monitor: ## Run the terminal monitor against the local simulator (SPEED=10)
	cargo run -q -p monitor -- --time-scale $(SPEED)

probe: ## Run the line-based probe against the local simulator (SPEED=10)
	cargo run -q -p probe -- --time-scale $(SPEED)
