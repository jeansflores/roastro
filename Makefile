.DEFAULT_GOAL := help

SPEED ?= 10

.PHONY: help build test lint fmt run-sim monitor probe

help: ## Show this help
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  %-10s %s\n", $$1, $$2}'

build: ## Build the Go tools into ./bin and the Rust binaries into ./target/release
	go build -o bin/ ./cmd/...
	cargo build --workspace --release

test: ## Run all Go (race detector) and Rust tests
	go test -race ./...
	cargo test --workspace

lint: ## Run golangci-lint, rustfmt and clippy
	golangci-lint run
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format code
	golangci-lint fmt

run-sim: ## Run the simulator with the autopilot (SPEED=10)
	cargo run -q -p sim -- --auto --speed $(SPEED)

monitor: ## Run the terminal monitor against the local simulator (SPEED=10)
	go run ./cmd/monitor --time-scale $(SPEED)

probe: ## Run the line-based probe against the local simulator (SPEED=10)
	cargo run -q -p probe -- --time-scale $(SPEED)
