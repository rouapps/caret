# Contributing to Caret

Thank you for your interest in contributing to Caret! This document provides guidelines for contributing.

## Development Setup

```bash
# Clone the repository
git clone https://github.com/rayanouaddi/caret.git
cd caret

# Build the project
cargo build

# Run in release mode (recommended for large datasets)
cargo build --release
```

## Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture
```

## Code Style

Before submitting a PR, please ensure your code passes these checks:

```bash
# Format code
cargo fmt

# Run linter (CI enforces zero warnings)
cargo clippy -- -D warnings
```

## Pull Request Process

1. Fork the repository and create your branch from `main`
2. Make your changes with clear, descriptive commits
3. Add tests for new functionality
4. Ensure `cargo test` and `cargo clippy` pass
5. Update documentation if needed
6. Open a PR with a clear description of your changes

## Reporting Issues

- Use GitHub Issues to report bugs
- Include your OS, Rust version, and steps to reproduce
- For large datasets, include file size and format (JSONL/Parquet/CSV)

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
