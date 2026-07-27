# Joy

> sculpting Dart/Flutter toolchains with light

[![CI](https://github.com/organiqlabs/joy/actions/workflows/ci.yml/badge.svg)](https://github.com/organiqlabs/joy/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Joy** is a CLI tool for managing Dart and Flutter SDK toolchains. It handles
downloading, installing, and switching between SDK versions — similar to
`rustup` but for the Dart/Flutter ecosystem.

## Features

- Install and manage multiple Dart/Flutter SDK versions via release archives or
  lightweight Git worktrees with a shared object cache
- Release channel tracking (stable, beta, dev, main)
- Directory-specific version overrides (project pinning)
- SHA256 integrity verification for downloaded artifacts
- Automatic toolchain discovery from overrides → `.joy.json` → global default
- Shared engine cache across installations (saves disk space)
- Shell completions for bash, zsh, fish, PowerShell, elvish
- Garbage collection for cached artifacts and Git objects

## Quick Start

```bash
# Install the latest stable Flutter
joy toolchain install stable

# List installed versions
joy toolchain list

# Set a global default
joy default stable

# Use it
flutter --version
```

## Commands

| Command                                         | Description                                                        |
| ----------------------------------------------- | ------------------------------------------------------------------ |
| `joy toolchain install <version>`               | Install a Flutter SDK toolchain (e.g., `3.29.0`, `stable`, `beta`) |
| `joy toolchain install <version> --git`         | Install via shallow Git clone (lightweight worktree)               |
| `joy toolchain list`                            | List installed toolchains                                          |
| `joy toolchain remove <version> [<version>...]` | Remove one or more installed toolchains                            |
| `joy toolchain update`                          | Upgrade the active toolchain to the latest on its channel          |
| `joy releases`                                  | List available Flutter releases from Google's storage API          |
| `joy default [<version>]`                       | Show or set the global default toolchain                           |
| `joy override set <version>`                    | Pin a specific version for the current directory                   |
| `joy override list`                             | List active directory overrides                                    |
| `joy doctor`                                    | Check that joy is set up correctly                                 |
| `joy gc`                                        | Run garbage collection on unused cached artifacts                  |
| `joy completions generate <shell>`              | Print a completion script for the given shell                      |
| `joy completions install [<shell>]`             | Install shell completions system-wide                              |

### Install Options

```
--force                   Re-download even if cached
--git                     Clone from Git repo using shared object cache
--repo <URL>              Git remote URL (default: flutter/flutter)
--profile <minimal|default|full>
                          Installation profile:
                            minimal — SDK only (no engine)
                            default — SDK + host engine
                            full    — SDK + engine + all platform artifacts
--skip-checksum           Skip SHA256 verification after download
```

### Garbage Collection

```
joy gc                    Show cache sizes
joy gc --git              Also clean the shared Git object cache
joy gc --engines          Also clean the shared engine cache
```

## Directory Layout

Joy follows the XDG Base Directory Specification:

```
~/.local/share/joy/          XDG data home
├── envs/                    Installed Flutter SDK versions
│   ├── 3.29.0/
│   │   ├── bin/             Flutter & Dart binaries
│   │   ├── .git             Git worktree linking (git installs)
│   │   └── .profile         Installation profile sidecar
│   └── ...
├── default                  Symlink to the global default version
└── ...

~/.cache/joy/                XDG cache home
├── engines/                 Shared engine binary cache
├── git/                     Shared bare Git repository (object cache)
│   └── worktrees/           Lightweight worktree metadata
├── releases/                Cached release list (per-platform JSON)
└── tmp/                     Temporary download artifacts
```

Project-specific configuration is stored in `.joy.json` (version pinning) or
`.joy/override` (directory override).

## Version Resolution

When resolving the active toolchain, joy checks in this order:

1. **Directory override** — `.joy/override` file in current or parent directory
2. **Project config** — `.joy.json` in current or parent directory
3. **Global default** — Symlink at `~/.local/share/joy/default`

## Environment Variables

| Variable         | Purpose                                             |
| ---------------- | --------------------------------------------------- |
| `XDG_DATA_HOME`  | Override data directory (default: `~/.local/share`) |
| `XDG_CACHE_HOME` | Override cache directory (default: `~/.cache`)      |
| `SHELL`          | Auto-detection for shell completions                |

## License

Licensed under the [MIT License](LICENSE).
