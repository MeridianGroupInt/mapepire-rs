# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial Rust crate scaffolding: `Cargo.toml` manifest with v0.1
  dependencies (`serde`, `serde_json`, `thiserror`, `zeroize`), public
  module structure (`config`, `error`, `password`, `protocol`),
  placeholder modules for forthcoming work, and `.gitignore`.
- Profile tuning: release builds use fat LTO + single codegen unit +
  panic-abort + strip; dev disables optimization; test bumps to opt-level 1
  for faster test cycles.

[Unreleased]: https://github.com/MeridianGroupInt/mapepire-rs/compare/HEAD...HEAD
