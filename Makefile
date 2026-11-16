.DEFAULT_GOAL := help

SPEED ?= 10
MAP ?= configs/regmap.json

.PHONY: help build test ui-test lint fmt desktop run run-sim monitor probe

help: ## Show this help
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  %-10s %s\n", $$1, $$2}'

build: ## Build the release binaries into ./target/release
	cargo build --workspace --release

test: ## Run all tests
	cargo test --workspace

ui-test: ## Run the desktop app's headless UI tests
	cargo test -p desktop --features ui-test

lint: ## Check formatting and run clippy with warnings as errors
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format code
	cargo fmt --all

desktop: ## Run the desktop app with its built-in simulator (a demo)
	cargo run -q -p desktop --release -- --simulator

run: ## Run Roastro against the local simulator, recording into ./roastro.db (SPEED=10, MAP=...)
	cargo run -q -p roastro -- --time-scale $(SPEED) --map $(MAP) --db roastro.db

run-sim: ## Run the simulator with the autopilot (SPEED=10, MAP=configs/regmap.json)
	cargo run -q -p sim -- --auto --speed $(SPEED) --map $(MAP)

monitor: ## Run the terminal monitor against the local simulator (SPEED=10, MAP=...)
	cargo run -q -p monitor -- --time-scale $(SPEED) --map $(MAP)

probe: ## Run the line-based probe against the local simulator (SPEED=10, MAP=...)
	cargo run -q -p probe -- --time-scale $(SPEED) --map $(MAP)
