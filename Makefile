.PHONY: help setup build build-release test test-min test-doc lint format format-check fix audit deny coverage fuzz outdated msrv-check doc doc-open doc-private ci pre-commit pre-pr release-check

# Default goal — keep it informative.
.DEFAULT_GOAL := help

help:
	@echo "Available targets (run 'make <target>'):"
	@echo ""
	@echo "  setup           Install required cargo tools (one-time)"
	@echo ""
	@echo "  Build & test:"
	@echo "    build         cargo build (debug)"
	@echo "    build-release cargo build --release"
	@echo "    test          cargo test --all-features"
	@echo "    test-min      cargo test --no-default-features (verifies feature gating)"
	@echo "    test-doc      cargo test --doc"
	@echo ""
	@echo "  Code quality:"
	@echo "    lint          cargo clippy with -D warnings"
	@echo "    format        cargo +nightly fmt --all"
	@echo "    format-check  cargo +nightly fmt --all -- --check"
	@echo "    fix           cargo clippy --fix"
	@echo ""
	@echo "  Supply chain:"
	@echo "    audit         cargo audit (vulnerabilities)"
	@echo "    deny          cargo deny check (licenses, bans, advisories)"
	@echo "    outdated      cargo outdated --workspace --root-deps-only"
	@echo ""
	@echo "  Coverage & fuzz:"
	@echo "    coverage      cargo llvm-cov --all-features (HTML report in target/llvm-cov)"
	@echo "    fuzz          cargo fuzz run codec_response (Ctrl-C to stop)"
	@echo ""
	@echo "  Docs:"
	@echo "    doc           cargo doc --no-deps --all-features"
	@echo "    doc-open      ... and open in browser"
	@echo "    doc-private   ... including private items"
	@echo ""
	@echo "  MSRV:"
	@echo "    msrv-check    cargo check pinned to rust-version from Cargo.toml"
	@echo ""
	@echo "  Workflows:"
	@echo "    pre-commit    format + fix (run before each commit)"
	@echo "    pre-pr        format-check + lint + test + audit + deny + doc (run before opening PR)"
	@echo "    ci            What CI runs on every PR"
	@echo "    release-check pre-pr + cargo publish --dry-run + CHANGELOG check"

# --- Setup ---------------------------------------------------------------

setup:
	@echo "Installing cargo tools…"
	cargo install --locked cargo-audit cargo-deny cargo-llvm-cov cargo-outdated cargo-fuzz cargo-msrv 2>&1 | tail -20
	@echo "Installing nightly toolchain for rustfmt unstable options…"
	rustup toolchain install nightly --component rustfmt
	@echo "Done. Now run 'make pre-pr' to verify the workspace."

# --- Build ---------------------------------------------------------------

build:
	cargo build --all-features

build-release:
	cargo build --release --all-features

# --- Test ----------------------------------------------------------------

test:
	cargo test --all-features

test-min:
	cargo test --no-default-features

test-doc:
	cargo test --doc --all-features

# --- Code quality --------------------------------------------------------

lint:
	cargo clippy --all-targets --all-features -- -D warnings

format:
	cargo +nightly fmt --all

format-check:
	cargo +nightly fmt --all -- --check

fix:
	cargo clippy --all-targets --all-features --fix --allow-dirty -- -D warnings

# --- Supply chain --------------------------------------------------------

audit:
	cargo audit

deny:
	cargo deny check

outdated:
	cargo outdated --workspace --root-deps-only

# --- Coverage & fuzz -----------------------------------------------------

coverage:
	cargo llvm-cov --all-features --html
	@echo "HTML report at target/llvm-cov/html/index.html"

coverage-ci:
	cargo llvm-cov --all-features --lcov --output-path lcov.info

fuzz:
	cargo fuzz run codec_response -- -max_total_time=60

# --- Documentation -------------------------------------------------------

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

doc-open:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --open

doc-private:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --document-private-items

# --- MSRV ----------------------------------------------------------------

msrv-check:
	@MSRV=$$(grep '^rust-version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/'); \
	echo "Checking against MSRV $$MSRV…"; \
	rustup toolchain install $$MSRV >/dev/null 2>&1 || true; \
	cargo +$$MSRV check --all-features

# --- Workflows -----------------------------------------------------------

pre-commit: format fix

pre-pr: format-check lint test audit deny doc
	@echo "All pre-PR checks passed."

ci: format-check lint test test-min doc

release-check: pre-pr
	cargo publish --dry-run --allow-dirty
	@echo ""
	@echo "Release-check passed. Verify CHANGELOG has an entry for the new version, then tag."
