# Joy

> sculpting Dart/Flutter toolchains with light

[![CI](https://github.com/organiqlabs/joy/actions/workflows/ci.yml/badge.svg)](https://github.com/organiqlabs/joy/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Joy** is a CLI tool for managing Dart and Flutter SDK toolchains. It handles downloading, installing, and switching between SDK versions — similar to `rustup` but for the Dart/Flutter ecosystem.

## Features

- Install and manage multiple Dart/Flutter SDK versions
- Automatic toolchain discovery from `pubspec.yaml` and project configuration
- SHA256 integrity verification for downloaded artifacts
- Release channel tracking (stable, beta, dev, main)
- Fast, parallel downloads with progress indication

## Usage

```
Usage: joy <COMMAND>

Commands:
  install       Install a toolchain
  toolchain     Manage SDK toolchains
  completions   Generate shell completions
  help          Print this message or the help of the given subcommand(s)
```

## License

Licensed under the [MIT License](LICENSE).
