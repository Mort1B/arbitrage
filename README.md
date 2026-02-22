# Arbitrage (Triangle Arbitrage Signal Scanner)

Rust service that:

- streams Binance depth updates for many pairs
- evaluates configured triangular arbitrage paths
- serves live data over WebSocket
- writes structured opportunity signals to JSONL files
- provides an offline analyzer for ranking triangles

This is currently a signal/research tool, not an order-executing bot.

## Features

- Async Binance websocket ingestion (`tokio-tungstenite`)
- Multi-triangle evaluation from `config.yaml`
- WebSocket server for live consumers (`/ws`)
- JSONL signal logging for offline analysis / downstream tools
- Built-in `analyze` command for quick summaries
- Built-in `pnl-report` command for positive-PnL aggregation (with CSV export)
- Built-in `simulate` paper-trading command with virtual balances
- Auto triangle generation from an asset list (`exchangeInfo` graph + 3-cycle enumeration)
- `rust_decimal`-based execution/profit math in the triangle engine
- Auto-reconnect to Binance with configurable backoff
- Backpressure-aware client broadcasting

## Requirements

- Rust (stable)
- `cargo`

## Run

Start the live service:

```bash
cargo run
```

By default it:

- reads `config.yaml`
- writes signals to `data/triangle_signals.jsonl`
- serves websocket on `ws://127.0.0.1:8000/ws`

## Auto-Generate Triangles From Asset List

You can enable automatic triangle generation from an asset universe using Binance `exchangeInfo`.

When enabled, the program will:

1. Load Binance exchange metadata
2. Build a tradable asset graph (spot symbols only, `TRADING` status)
3. Enumerate valid 3-cycles
4. Generate `triangles`
5. Generate the minimal `depth_streams` set required
6. Optionally merge Binance lot-size / min-notional filters into `exchange_rules.pair_rules`

Config example (`config.yaml`):

```yaml
auto_triangle_generation:
  enabled: true
  exchange_info_url: https://api.binance.com/api/v3/exchangeInfo
  include_reverse_cycles: true
  include_all_starts: false
  max_triangles: 0
  merge_pair_rules_from_exchange_info: true
  assets: [btc, eth, bnb, usdt, usdc, xrp, sol]
```

Notes:

- `max_triangles: 0` means no limit
- `include_reverse_cycles` evaluates both directions of a cycle
- `include_all_starts` expands each cycle into all 3 starting assets (higher CPU load)

### Generate Config Preview File

To inspect the auto-generated universe before running live:

```bash
cargo run -- generate-config generated_triangles.yaml
```

Options:

- `--no-pair-rules` to omit extracted Binance filter rules from the output file
- `--stdout` / `--print-only` to write the generated YAML to stdout instead of a file

The generated file contains:

- `depth_streams`
- `triangles`
- optional `pair_rules`
- summary metadata (counts, timestamp)

## Analyze Recorded Signals

Analyze the default signal file:

```bash
cargo run -- analyze
```

Analyze a custom JSONL file:

```bash
cargo run -- analyze /path/to/signals.jsonl
```

The analyzer prints per-triangle summary stats such as:

- sample count
- worthy count
- average / max best profit (bps)
- average hit rate

## Positive PnL Report (Offline)

Find triangles with positive estimated PnL from recorded signals:

```bash
cargo run -- pnl-report
```

Examples:

```bash
cargo run -- pnl-report data/triangle_signals.jsonl --top 25 --min-adjusted-bps 3.0
cargo run -- pnl-report --include-unworthy --include-nonexecutable
cargo run -- pnl-report --top 50 --csv pnl_top50.csv
```

This report aggregates by triangle and shows:

- sample count
- positive adjusted-PnL sample count
- summed estimated raw/adjusted PnL (in the triangle start asset)
- average and max adjusted bps

Optional export:

- `--csv <path>` writes the displayed top rows to a CSV file

## Paper Trade Simulation

Run the offline paper-trade simulator on recorded signals:

```bash
cargo run -- simulate
```

Custom file:

```bash
cargo run -- simulate /path/to/signals.jsonl
```

Optional tuning:

```bash
cargo run -- simulate data/triangle_signals.jsonl --cooldown-ms 3000 --min-adjusted-bps 5.0
```

With virtual balances + position sizing:

```bash
cargo run -- simulate data/triangle_signals.jsonl \
  --balance usdt=10000 --balance btc=0.25 \
  --position-size-pct 0.20 \
  --max-position-vs-signal 1.0
```

Flags:

- `--cooldown-ms N`: minimum time between repeated trades on the same triangle
- `--min-adjusted-bps X`: extra filter on top of the logged signal
- `--include-unworthy`: simulate all executable signals, not only `worthy=true`
- `--balance asset=amount`: seed a virtual balance (repeatable)
- `--position-size-pct P`: fraction of available balance used per simulated trade (`0 < P <= 1`)
- `--max-position-vs-signal M`: cap position size to `M * assumed_start_amount` from the signal
- `--no-auto-seed`: disable automatic seeding of unseen assets
- `--seed-multiplier M`: when auto-seeding, seed unseen assets with `M * assumed_start_amount`

Simulator output includes:

- executed trade count
- win/loss count
- average adjusted bps
- estimated PnL grouped by start asset
- top triangles by average adjusted bps
- final virtual balances by asset

## Configuration (`config.yaml`)

Important keys:

- `bind_addr`: websocket server bind address
- `bind_port`: websocket server port
- `reconnect_delay_ms`: Binance reconnect backoff
- `update_interval`: publish interval (ms)
- `results_limit`: Binance depth levels requested
- `depth_streams`: subscribed Binance depth streams
- `triangles`: triangle definitions (`parts` + `pairs`)
- `auto_triangle_generation`: build triangles/depth streams automatically from an asset list
- `signal_log_enabled`: enable JSONL logging
- `signal_log_path`: output JSONL path
- `signal_log_channel_capacity`: buffered log queue size
- `signal_min_profit_bps`: threshold for marking a signal as `worthy`
- `signal_min_hit_rate`: threshold for marking a signal as `worthy`
- `exchange_rules`: execution filters and assumptions (min notional, lot size, fees, assumed start amount)

Precision note:

- `exchange_rules` numeric values are parsed into `Decimal` for execution-grade calculations
- YAML values can be written as normal numbers (for example `7.5`) and are parsed precisely into decimal values
- Logged signal numeric fields are serialized from `Decimal` (JSON string values) to preserve precision across tooling

## Signal Output Format (JSONL)

Each line in `signal_log_path` is a JSON object (`TriangleOpportunitySignal`) with fields including:

- `timestamp_ms`
- `exchange`
- `triangle_parts`
- `triangle_pairs`
- `depth_levels_considered`
- `profitable_levels`
- `hit_rate`
- `top_profit_bps`
- `best_profit_bps`
- `avg_profit_bps`
- `best_level_index`
- `worthy`
- `best_level_quotes` (ask/bid + size for each leg)

Numeric precision:

- Decimal-valued metrics and quotes are written as JSON strings (for example `"best_profit_bps":"12.3456"`)
- Offline tools in this repo (`analyze`, `pnl-report`, `simulate`) read this format directly

This format is intended to be consumed later by:

- dashboards
- paper-trading simulators
- candidate bot execution logic

## WebSocket Output

The websocket endpoint (`/ws`) broadcasts triangle payloads containing:

- triangle parts
- profit vector across evaluated depth levels
- raw depth snapshots for the three triangle legs

Clients can also send `ping` and receive `pong`.

## Project Layout

- `src/main.rs` - startup, config, reconnect loop, websocket server
- `src/workers.rs` - Binance ingestion, triangle computation, broadcasting, signal generation
- `src/ws.rs` - client websocket connection handling
- `src/models.rs` - Binance depth and signal data models
- `src/signal_log.rs` - async JSONL writer task
- `src/analyzer.rs` - offline analyzer CLI
- `src/auto_triangles.rs` - asset-universe graph builder + triangle enumeration
- `src/generate_config.rs` - writes auto-generated triangles/depth streams to YAML
- `src/pnl_report.rs` - positive-PnL opportunity report from signal logs
- `src/config.rs` - config structs + defaults

## Notes / Limitations

- Core triangle/execution math uses `rust_decimal`
- Signal log numeric fields are emitted as decimal strings for precision-preserving storage
- Console reports still format values for readability (some aggregates are displayed as rounded decimals)
- Does not place orders
- Does not model slippage, fees per symbol/tier, min notional, balances, or risk limits
- `worthy` is a heuristic threshold, not a trading decision

## Next Steps (Recommended)

1. Add Binance `exchangeInfo` cache/persistence to avoid fetching on every startup in auto-generation mode.
2. Add a live paper-trading runtime that consumes signals directly (not only offline replay).
3. Move simulator/report internal aggregation math from `f64` to `Decimal` if you want precision-preserving analytics end-to-end.
4. Add execution adapter interfaces before implementing a real bot.
