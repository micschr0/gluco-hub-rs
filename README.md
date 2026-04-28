# cgm-bridge

Rust service that polls LibreLink Up and exposes glucose readings via HTTP, with optional pushes to Nightscout.

## Quick start

```bash
cp config.example.toml config.toml
# edit config.toml and export ENV variables for secrets
cargo run --release -- run
```

## Endpoints

- `GET /glucose/latest` — last known reading
- `GET /healthz` — liveness probe
- `GET /metrics` — Prometheus metrics

## Configuration

TOML file with ENV-variable references for secrets. See `config.example.toml`.

## Documentation

- `CLAUDE.md` — agent context and conventions
- `docs/ARCHITECTURE.md` — design and diagrams

## License

TBD
