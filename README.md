# cargo-manage

Manage Cargo.toml dependencies across Rust workspaces.

## Installation

```bash
cargo install cargo-manage
```

## Commands

### `cargo manage` / `cargo manage deps`

Hoist dependency versions to `workspace.dependencies` and update member crates to use `workspace = true`.

```bash
cargo manage                    # run in workspace root
cargo manage --dry-run          # preview changes
cargo manage -r /path/to/ws     # specify workspace root
```

### `cargo manage sort`

Sort dependencies alphabetically in all Cargo.toml files. Also sorts `workspace.members`.

```bash
cargo manage sort
cargo manage sort --prefix mycompany   # sort mycompany-* deps first
```

### `cargo manage restore`

Restore Cargo.toml files from `.bak` backups created by previous operations.

```bash
cargo manage restore
cargo manage restore --dry-run
```

## Options

| Flag | Description |
|------|-------------|
| `-r, --root <PATH>` | Workspace root directory (default: `.`) |
| `--dry-run` | Preview changes without modifying files |
| `-v, --verbose` | Enable debug output |
| `--prefix <PREFIX>` | Priority prefix for sorting |

## Behavior

- Creates `.bak` backups before modifying files
- Skips path dependencies (local crates)
- Preserves TOML formatting, comments, and key order
- Ignores `target/`, `.git/`, `node_modules/`, `vendor/`

## License

MIT OR Apache-2.0
