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
  exchange_info_cache_enabled: true
  exchange_info_cache_path: data/cache/binance_exchange_info.json
  exchange_info_cache_ttl_secs: 300
  include_reverse_cycles: true
  include_all_starts: false
  max_triangles: 0
  merge_pair_rules_from_exchange_info: true
  assets: [btc, eth, bnb, usdt, usdc, xrp, sol]
```

Notes:

- `max_triangles: 0` means no limit
- `exchange_info_cache_*` reduces repeated startup fetches during auto-generation
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
  - includes `exchange_info_cache_enabled`, `exchange_info_cache_path`, `exchange_info_cache_ttl_secs`
- `signal_log_enabled`: enable JSONL logging
- `signal_log_path`: output JSONL path
- `signal_log_channel_capacity`: buffered log queue size
- `signal_min_profit_bps`: threshold for marking a signal as `worthy`
- `signal_min_hit_rate`: threshold for marking a signal as `worthy`
- `max_book_age_ms`: freshness gate for triangle legs (0 disables age gating)
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
- `book_receive_timestamp_ms_by_leg`
- `book_age_ms_by_leg`
- `min_book_age_ms`
- `max_book_age_ms`
- `book_freshness_passed`
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
- `src/binance_rest.rs` - REST helpers (exchangeInfo cache, depth snapshot fetch for upcoming local-book sync)
- `src/generate_config.rs` - writes auto-generated triangles/depth streams to YAML
- `src/pnl_report.rs` - positive-PnL opportunity report from signal logs
- `src/config.rs` - config structs + defaults

## Notes / Limitations

- Core triangle/execution math uses `rust_decimal`
- Signal log numeric fields are emitted as decimal strings for precision-preserving storage
- Offline tools (`analyze`, `pnl-report`, `simulate`) use `Decimal` internally for signal-derived calculations/aggregates
- `worthy` now also depends on market-data freshness (`max_book_age_ms`) in addition to profit/hit-rate/execution filters
- Console reports still format values for readability (some values are displayed as rounded decimals)
- Does not place orders
- Does not model slippage, fees per symbol/tier, min notional, balances, or risk limits
- `worthy` is a heuristic threshold, not a trading decision

## Next Steps (Recommended)

1. Add Binance `exchangeInfo` cache/persistence to avoid fetching on every startup in auto-generation mode.
2. Add a live paper-trading runtime that consumes signals directly (not only offline replay).
3. Add decimal-preserving CSV/report export modes for all analytics outputs (not only console summaries).
4. Add execution adapter interfaces before implementing a real bot.

## Engineering Plan (Concrete)

This plan is focused on turning the current signal scanner into an execution-ready research platform, then a safe paper-trading runtime, and only then a constrained live bot.

### Phase 1: Execution-Ready Research (Priority)

Goal: make opportunity detection market-data-correct and latency-aware before any live execution work.

Scope:

1. Build local order books per symbol using Binance diff-depth + REST snapshot sync.
2. Detect sequence gaps and automatically resync stale/broken books.
3. Gate signal generation on order book health/staleness.
4. Add timing metrics (receive lag / processing lag / publish lag).
5. Tighten simulator assumptions with latency/slippage hooks.

Planned modules / files:

- Add `src/orderbook.rs`
  - local book state (`bids`, `asks`, last update IDs)
  - apply diff updates
  - snapshot sync / reset helpers
  - book staleness and health status
- Add `src/binance_rest.rs`
  - snapshot fetch for `/api/v3/depth`
  - `exchangeInfo` fetch/cache helpers (can reuse in auto-triangle generation)
  - server time helper (future timing sync)
- Add `src/market_state.rs`
  - registry of all symbol order books
  - update routing by stream/symbol
  - health view used by workers
- Refactor `src/workers.rs`
  - consume local books instead of direct depth payload snapshots
  - use best executable levels from local book state
  - reject/pause triangles when any leg book is stale or unsynced
  - add latency/staleness fields to signals
- Update `src/models.rs`
  - add signal fields for data age and decision latency
  - optional signal reason fields for stale-book rejection
- Update `src/config.rs`
  - add book sync/staleness thresholds (ms)
  - add REST snapshot depth size config
  - add optional slippage/latency modeling knobs for simulator
- Update `src/main.rs`
  - initialize shared market state and REST helpers
  - wire worker to market state + reconnect-safe sync logic

Phase 1 acceptance criteria:

- No triangle is evaluated unless all 3 legs are in `synced` state.
- Sequence gaps trigger resync without crashing the process.
- Signals include enough timing metadata to analyze edge decay.
- A test/replay path exists for validating order book merge logic.

### Phase 2: Live Paper-Trading Runtime

Goal: simulate execution continuously from live market data with realistic state transitions (still no real orders).

Scope:

1. Add a runtime paper executor that consumes live signals (not offline JSONL only).
2. Add slippage/residual inventory modeling per leg.
3. Add balance/exposure limits and kill switch controls.
4. Add realized-vs-estimated PnL attribution metrics.

Planned modules / files:

- Add `src/paper_runtime.rs`
  - subscribes to internal signal stream
  - executes paper trades with cooldown/risk constraints
  - updates virtual balances continuously
- Add `src/risk.rs`
  - notional caps, asset exposure caps, kill switch, loss streak limits
- Add `src/slippage.rs`
  - configurable slippage model (bps and/or depth-walk based)
- Refactor `src/simulator.rs`
  - share execution logic with `paper_runtime` where possible
  - extract common simulation primitives
- Update `src/config.rs`
  - paper runtime enable/disable and risk thresholds
- Update `src/main.rs`
  - spawn paper runtime task when enabled

Phase 2 acceptance criteria:

- Live paper runtime can run for extended periods without unbounded memory growth.
- Realized paper PnL is tracked separately from estimated signal PnL.
- Risk rules can prevent trades and emit explicit rejection reasons.

### Phase 3: Exchange Execution Foundation (No Aggressive Live Trading Yet)

Goal: build safe execution plumbing and reconciliation before enabling meaningful capital.

Scope:

1. Signed REST client for order placement/cancel/query.
2. User Data Stream consumer and order state machine.
3. Exchange/account commission and filter reconciliation.
4. Idempotent client order IDs and recovery/restart reconciliation.

Planned modules / files:

- Add `src/execution/mod.rs`
- Add `src/execution/binance_client.rs`
  - signed requests, time sync, recvWindow handling, rate limit backoff
- Add `src/execution/order_manager.rs`
  - order lifecycle, retries, cancel/replace, partial fill handling
- Add `src/user_stream.rs`
  - listen key lifecycle
  - execution reports / balance updates
- Add `src/reconcile.rs`
  - startup reconciliation of open orders + balances
- Update `src/models.rs`
  - internal order/fill/balance domain structs (separate from signal structs)
- Update `src/config.rs`
  - API credentials via env/config references, execution limits, dry-run mode

Phase 3 acceptance criteria:

- Order state is recoverable after process restart.
- Partial fills and cancellations are reconciled against exchange truth.
- Execution can run in dry-run mode with full audit logs.

### Phase 4: Controlled Live Trading (Small Capital, Narrow Universe)

Goal: limited live rollout with strict controls and observability.

Scope:

1. Enable live execution for a small asset subset and strict notional caps.
2. Monitor realized vs expected edge and reject rates.
3. Tune latency/risk thresholds from production observations.

Operational focus:

- start with few triangles, high liquidity pairs only
- low max notional and max daily loss
- immediate kill switch path (config + runtime)
- audit logs for every decision/order/fill event

### Cross-Cutting Work (can happen in parallel)

1. Add replay tooling from recorded market data (`data/` -> deterministic worker replay).
2. Add integration tests around websocket + signal pipeline + paper runtime.
3. Add metrics export (`Prometheus`-style) for latency, staleness, opportunities, rejections.
4. Add `exchangeInfo` caching (file + TTL) for startup speed and deterministic runs.

### Suggested Immediate Implementation Order (for this repo)

1. `src/orderbook.rs` + unit tests (book merge correctness first).
2. `src/market_state.rs` + worker integration (replace direct depth snapshot use).
3. Staleness gating + latency fields in `TriangleOpportunitySignal`.
4. Replay test harness for market-data correctness.
5. Live paper runtime (`src/paper_runtime.rs`) reusing simulator logic.
