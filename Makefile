# Task runner for the Rust CLI tool workspace.
# Targets are self-documented via the trailing "## description" comment
# (see the `help` target). Written for GNU Make 3.81 (macOS default):
# no .RECIPEPREFIX, no $(file ...), plain tab-indented recipes only.

.DEFAULT_GOAL := help

INSTALL_ROOT ?= $(HOME)/.local

.PHONY: help build release test lint fmt fmt-check check install install-all new clean dist-build dist-tag

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

dist-build: ## Package a release tarball into dist/: make dist-build TOOL=<name>
	@if [ -z "$(TOOL)" ]; then \
		echo "error: TOOL is required. usage: make dist-build TOOL=<name>" >&2; \
		exit 1; \
	fi
	@VERSION=$$(grep -m1 '^version = ' tools/$(TOOL)/Cargo.toml | sed -E 's/version = "(.*)"/\1/'); \
	if [ -z "$$VERSION" ]; then \
		echo "error: could not read version from tools/$(TOOL)/Cargo.toml" >&2; \
		exit 1; \
	fi; \
	cargo build --release -p $(TOOL) --target aarch64-apple-darwin; \
	mkdir -p dist; \
	tar czf "dist/$(TOOL)-$$VERSION-aarch64-apple-darwin.tar.gz" -C target/aarch64-apple-darwin/release $(TOOL); \
	echo "created dist/$(TOOL)-$$VERSION-aarch64-apple-darwin.tar.gz"

dist-tag: ## Tag + push a release: make dist-tag TOOL=<name> (needs clean tree on main)
	@if [ -z "$(TOOL)" ]; then \
		echo "error: TOOL is required. usage: make dist-tag TOOL=<name>" >&2; \
		exit 1; \
	fi
	@if [ -n "$$(git status --porcelain)" ]; then \
		echo "error: working tree is not clean; commit or stash changes first" >&2; \
		exit 1; \
	fi
	@branch=$$(git rev-parse --abbrev-ref HEAD); \
	if [ "$$branch" != "main" ]; then \
		echo "error: dist-tag must run on main (current branch: $$branch)" >&2; \
		exit 1; \
	fi
	@VERSION=$$(grep -m1 '^version = ' tools/$(TOOL)/Cargo.toml | sed -E 's/version = "(.*)"/\1/'); \
	if [ -z "$$VERSION" ]; then \
		echo "error: could not read version from tools/$(TOOL)/Cargo.toml" >&2; \
		exit 1; \
	fi; \
	TAG="$(TOOL)-v$$VERSION"; \
	git tag "$$TAG"; \
	git push origin "$$TAG"; \
	echo "tagged and pushed $$TAG"
