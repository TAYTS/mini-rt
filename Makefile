.PHONY: test test-verbose fmt lint build run clean

# Run all unit test and integration tests
test:
	cargo test

# Run tests with verbose output
test-verbose:
	cargo test -- --nocapture

# Run formatter
fmt:
	cargo fmt

# Run linter
lint:
	cargo clippy

build:
	cargo build

run:
	cargo run

# Clean build artifacts
clean:
	cargo clean

