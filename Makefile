# Task runner for the Rust CLI tool workspace.
# Targets are self-documented via the trailing "## description" comment
# (see the `help` target). Written for GNU Make 3.81 (macOS default):
# no .RECIPEPREFIX, no $(file ...), plain tab-indented recipes only.

.DEFAULT_GOAL := help

INSTALL_ROOT ?= $(HOME)/.local

.PHONY: help build release test lint fmt fmt-check check install install-all new clean

help: ## Show this help message
	@echo "Available targets:"
	@grep -E '^[a-zA-Z0-9_-]+:.*##' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*##"}; {printf "  %-15s %s\n", $$1, $$2}'

build: ## Build all workspace crates (debug)
	cargo build --workspace

release: ## Build all workspace crates (release)
	cargo build --workspace --release

test: ## Run tests for all workspace crates
	cargo test --workspace

lint: ## Run clippy across the workspace, warnings as errors
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format all workspace crates
	cargo fmt --all

fmt-check: ## Check formatting without modifying files
	cargo fmt --all --check

check: fmt-check lint test ## Run fmt-check + lint + test (local pre-push gate)

install: ## Install one tool: make install TOOL=<name> [INSTALL_ROOT=path]
	@if [ -z "$(TOOL)" ]; then \
		echo "error: TOOL is required. usage: make install TOOL=<name>" >&2; \
		exit 1; \
	fi
	cargo install --path tools/$(TOOL) --root $(INSTALL_ROOT)

install-all: ## Install every tool under tools/
	@for dir in tools/*/; do \
		name=$$(basename "$$dir"); \
		echo "==> installing $$name"; \
		cargo install --path "$$dir" --root $(INSTALL_ROOT) || exit 1; \
	done

new: ## Scaffold a new tool from templates/: make new NAME=<name>
	@if [ -z "$(NAME)" ]; then \
		echo "error: NAME is required. usage: make new NAME=<name>" >&2; \
		exit 1; \
	fi
	@if [ -e "tools/$(NAME)" ]; then \
		echo "error: tools/$(NAME) already exists" >&2; \
		exit 1; \
	fi
	@mkdir -p "tools/$(NAME)/src"
	@sed 's/{{NAME}}/$(NAME)/g' templates/tool/Cargo.toml.tmpl > "tools/$(NAME)/Cargo.toml"
	@sed 's/{{NAME}}/$(NAME)/g' templates/tool/src/main.rs.tmpl > "tools/$(NAME)/src/main.rs"
	@echo "created tools/$(NAME)"

clean: ## Remove build artifacts
	cargo clean
