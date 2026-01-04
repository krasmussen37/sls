# SLS CLI Guide (Newer Commands)

This guide focuses on the newer operational commands: `discover`, `coverage`,
and `config`. It also documents the service catalog format used by `coverage`.

## Discover

`sls discover` scans the machine for likely log sources. It checks common
locations (journald, syslog files, agent logs under `~/.claude` and `~/.codex`)
and uses heuristics to score confidence.

```bash
# Show discoveries but do not auto-select
sls discover

# Auto-select sources with confidence >= 80
sls discover --auto
```

When `--auto` is used, high-confidence sources are marked selected and stored.
Without `--auto`, use `sls sources add` to register sources manually.

## Coverage

`sls coverage` compares three sets of data:

1. Services in the service catalog (`~/.sls/service_catalog.yaml`)
2. Discovered sources from `sls discover`
3. Indexed sources stored in the database

The command prints a summary plus a per-service status:

- `indexed` - at least one expected source is indexed
- `discovered` - found but not indexed yet
- `missing` - no matching sources found
- `uncataloged` - discovered source not listed in the catalog

### Initialize a sample catalog

```bash
sls coverage --init
```

This creates `~/.sls/service_catalog.yaml` with a starter set of services.
Edit that file to match your environment, then re-run `sls coverage`.

## Service catalog format

The catalog is a YAML map of service IDs to definitions.

```yaml
version: "1"
services:
  journald:
    name: "System Journal"
    description: "systemd journal for all system services"
    sources:
      - "journald"
    patterns: []
    tags: ["system"]
    required: true
  nginx:
    name: "Nginx"
    description: "Nginx web server logs"
    sources:
      - "/var/log/nginx/access.log"
      - "/var/log/nginx/error.log"
    required: false
default_tags: []
```

Fields:

- `sources` can be source IDs (like `journald`) or paths (like `/var/log/syslog`).
- `required: true` marks services that should always have logs; missing ones are
  called out in the coverage summary.

## Config

`sls config` manages `~/.sls/config.yaml`.

```bash
# Show all config values
sls config

# Read a single key
sls config discovery-mode

# Set a value
sls config discovery-mode daemon
```

Supported keys:

- `discovery-mode` - `manual`, `cron`, `daemon`, `hybrid`
- `cron-schedule` - crontab syntax (default `0 * * * *`)
- `daemon-interval` - seconds between scans (default `60`)
- `auto-accept-sources` - `true`/`false`
- `default-format` - `table`, `json`, `csv`
- `index.max-entries` - max rows (0 = unlimited)
- `index.retention-days` - retention window in days (0 = unlimited)
- `index.batch-size` - indexing batch size

When you change `discovery-mode`, SLS prints setup hints for cron or systemd.

## Notes on `sources`

`sls sources list` prints a quick view of configured sources.
`sls sources add` and `sls sources remove` are present but currently print
"not implemented" responses.
