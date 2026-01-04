# SLS (System Log Search)

SLS indexes system and application logs so you can search, triage, and
understand failures quickly. It is designed for local machines and agent
workflows, with commands for discovery, coverage, and configuration.

## Quick start

Build and run from source:

```bash
cargo run -- <command>
```

Common first steps:

```bash
# Discover log sources on this machine
sls discover --auto

# Index sources (one-shot)
sls index

# Search recent errors
sls search "error" --since 1h
```

## Key commands

- `index` - index logs from configured sources (`--full`, `--watch`)
- `search` - keyword search with filters (`--level`, `--service`, `--since`)
- `similar` - semantic search for related log patterns
- `context` - show surrounding entries for a timestamp or session
- `tail` - stream logs from a source in real time
- `discover` - scan the machine for log sources (`--auto`)
- `coverage` - compare discovered/indexed sources against a service catalog
- `config` - view and update SLS configuration
- `sources` - list/add/remove log sources (add/remove are stubbed today)
- `normalize` - convert arbitrary logs into the SLS schema
- `capabilities` - machine-readable capability listing
- `mcp` - run SLS as an MCP server

For usage details on the newer commands (`config`, `coverage`, `discover`),
see `docs/cli.md`.

## Data locations

SLS stores state under `~/.sls/`:

- `~/.sls/sls.db` - SQLite metadata database
- `~/.sls/config.yaml` - configuration
- `~/.sls/service_catalog.yaml` - service catalog used by `sls coverage`

## Output formats

Most commands support `--json` (or `--robot`) for machine-readable output.
`search` also supports `--format table|json|csv`.

## Configuration

Use the config command to view or set settings:

```bash
# Show all config
sls config

# Set discovery mode
sls config discovery-mode cron
```

See `docs/cli.md` for all keys and examples.
