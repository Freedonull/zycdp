# Contributing to chaser-oxide

Thank you for your interest in contributing to chaser-oxide! This document provides guidelines and instructions for contributing.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/YOUR_USERNAME/chaser-oxide.git`
3. Create a new branch: `git checkout -b feature/your-feature-name`

## Development Setup

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build the project
cargo build

# Run tests
cargo test --lib

# Run integration tests (requires Chrome/Chromium)
RUST_TEST_THREADS=1 cargo test --test '*'
```

## Project Structure

This is a Cargo workspace with multiple crates:
- `chaser-oxide` - Main library
- `chromiumoxide_cdp` - CDP protocol definitions
- `chromiumoxide_fetcher` - Browser binary fetcher
- `chromiumoxide_pdl` - PDL parser
- `chromiumoxide_types` - Shared types

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy --all` and fix any warnings
- Write tests for new functionality
- Keep commits focused and atomic

## Pull Request Process

1. Ensure all tests pass: `cargo test --lib`
2. Ensure CI checks pass: `cargo fmt --check && cargo clippy --all`
3. Update documentation if needed
4. Add a clear description of your changes
5. Reference any related issues

## Reporting Issues

When reporting issues, please include:
- Rust version (`rustc --version`)
- Chrome/Chromium version
- Operating system
- Steps to reproduce
- Expected vs actual behavior

## License

By contributing, you agree that your contributions will be licensed under the MIT OR Apache-2.0 license.
