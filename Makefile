.DEFAULT_GOAL := help

SPEED ?= 10

.PHONY: help build test lint fmt run-sim monitor probe

help: ## Show this help
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  %-10s %s\n", $$1, $$2}'

build: ## Build all binaries into ./bin
	go build -o bin/ ./cmd/...

test: ## Run all tests with the race detector
	go test -race ./...

lint: ## Run golangci-lint
	golangci-lint run

fmt: ## Format code
	golangci-lint fmt

run-sim: ## Run the simulator with the autopilot (SPEED=10)
	go run ./cmd/sim --auto --speed $(SPEED)

monitor: ## Run the terminal monitor against the local simulator (SPEED=10)
	go run ./cmd/monitor --time-scale $(SPEED)

probe: ## Run the line-based probe against the local simulator (SPEED=10)
	go run ./cmd/probe --time-scale $(SPEED)
