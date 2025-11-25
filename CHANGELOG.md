# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Updated `async-tungstenite` to 0.32.0
- Replace `winreg` by `windows-registry`

### Added

- Add option to disable automation detection
- Expose the `cmd` module for access to `CommandChain`

### Fixed

- Fixed typo in feature `_fetcher-rustls-tokio`
- More resilient message parsing, it should now not crash on unknown events coming from the browser
- Extensions should only be disabled when no extensions are provided

[Unreleased]: https://github.com/mattsse/chromiumoxide/compare/v0.7.0...HEAD
