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

## Configuration (`config.yaml`)

Important keys:

- `bind_addr`: websocket server bind address
- `bind_port`: websocket server port
- `reconnect_delay_ms`: Binance reconnect backoff
- `update_interval`: publish interval (ms)
- `results_limit`: Binance depth levels requested
- `depth_streams`: subscribed Binance depth streams
- `triangles`: triangle definitions (`parts` + `pairs`)
- `signal_log_enabled`: enable JSONL logging
- `signal_log_path`: output JSONL path
- `signal_log_channel_capacity`: buffered log queue size
- `signal_min_profit_bps`: threshold for marking a signal as `worthy`
- `signal_min_hit_rate`: threshold for marking a signal as `worthy`

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
- `src/config.rs` - config structs + defaults

## Notes / Limitations

- Uses `f64` math today (good for research, not ideal for execution precision)
- Does not place orders
- Does not model slippage, fees per symbol/tier, min notional, balances, or risk limits
- `worthy` is a heuristic threshold, not a trading decision

## Next Steps (Recommended)

1. Add paper-trading simulation using the JSONL signal stream.
2. Switch profit math to `rust_decimal` (or fixed-point).
3. Add exchange rule filters (min notional, lot size, fees, latency assumptions).
4. Add execution adapter interfaces before implementing a real bot.

