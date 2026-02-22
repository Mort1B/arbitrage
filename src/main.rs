use log::{debug, error, info, warn};
use std::{collections::HashMap, convert::Infallible, net::SocketAddr, sync::Arc};
use tokio::sync::{mpsc, RwLock};
use tokio::time::Duration;
use warp::{ws::Message, Filter, Rejection};

mod analyzer;
mod auto_triangles;
mod config;
mod generate_config;
mod handlers;
mod market_state;
mod models;
mod orderbook;
mod pnl_report;
mod signal_log;
mod simulator;
mod workers;
mod ws;

static BINANCE_WS_API: &str = "wss://stream.binance.com:9443";

#[derive(Debug, Clone)]
pub struct Client {
    pub sender: ClientSender,
}

type OutboundMessage = std::result::Result<Message, warp::Error>;
type ClientSender = mpsc::Sender<OutboundMessage>;
type SignalLogSender = mpsc::Sender<String>;
type Clients = Arc<RwLock<HashMap<String, Client>>>;
type Result<T> = std::result::Result<T, Rejection>;

fn get_binance_streams_url(
    depth_streams: &[String],
    update_interval: u32,
    results_limit: u32,
) -> String {
    let depth_streams_joined = depth_streams
        .iter()
        .map(|stream| format!("{stream}@depth{results_limit}@{update_interval}ms"))
        .collect::<Vec<_>>()
        .join("/");

    format!("{}/stream?streams={}", BINANCE_WS_API, depth_streams_joined)
}

#[tokio::main]
async fn main() {
    if analyzer::run_from_cli_args(std::env::args().skip(1)) {
        return;
    }
    if simulator::run_from_cli_args(std::env::args().skip(1)) {
        return;
    }
    if pnl_report::run_from_cli_args(std::env::args().skip(1)) {
        return;
    }
    if generate_config::run_from_cli_args(std::env::args().skip(1)).await {
        return;
    }

    log4rs::init_file("log_config.yaml", Default::default()).expect("failed to initialize logging");
    let config_file = std::fs::File::open("config.yaml").expect("Could not open config.yaml");
    let mut app_config: config::AppConfig =
        serde_yaml::from_reader(config_file).expect("Could not parse config.yaml");

    if app_config.auto_triangle_generation.enabled {
        info!(
            "Auto triangle generation enabled for {} assets",
            app_config.auto_triangle_generation.assets.len()
        );
        let generated = auto_triangles::generate_from_binance(&app_config.auto_triangle_generation)
            .await
            .expect("failed to generate triangles from Binance exchange info");

        info!(
            "Generated {} triangles using {} depth streams across {} assets",
            generated.triangles.len(),
            generated.depth_streams.len(),
            generated.used_assets.len()
        );

        app_config.depth_streams = generated.depth_streams;
        app_config.triangles = generated.triangles;

        if app_config
            .auto_triangle_generation
            .merge_pair_rules_from_exchange_info
        {
            merge_generated_pair_rules(
                &mut app_config.exchange_rules.pair_rules,
                generated.pair_rules,
            );
        }
    }
    let server_addr: SocketAddr = format!("{}:{}", app_config.bind_addr, app_config.bind_port)
        .parse()
        .expect("invalid bind_addr/bind_port in config.yaml");

    let clients: Clients = Arc::new(RwLock::new(HashMap::new()));

    info!("Configuring websocket route");
    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(with_clients(clients.clone()))
        .and_then(handlers::ws_handler);
    let routes = ws_route.with(warp::cors().allow_any_origin());

    let binance_url = get_binance_streams_url(
        &app_config.depth_streams,
        app_config.update_interval,
        app_config.results_limit,
    );

    let worker_clients = clients.clone();
    let worker_binance_url = binance_url.clone();
    let worker_config = app_config.clone();
    let signal_log_sender = if app_config.signal_log_enabled {
        Some(signal_log::spawn_jsonl_writer(
            app_config.signal_log_path.clone(),
            app_config.signal_log_channel_capacity.max(64),
        ))
    } else {
        None
    };
    tokio::spawn(async move {
        let reconnect_delay = Duration::from_millis(worker_config.reconnect_delay_ms.max(250));

        loop {
            info!("Connecting to Binance stream: {}", worker_binance_url);

            match tokio_tungstenite::connect_async(&worker_binance_url).await {
                Ok((socket, response)) => {
                    info!("Connected to Binance stream");
                    debug!("HTTP status code: {}", response.status());
                    for (header, header_value) in response.headers() {
                        debug!("header {}: {:?}", header, header_value);
                    }

                    workers::main_worker(
                        worker_clients.clone(),
                        worker_config.clone(),
                        socket,
                        signal_log_sender.clone(),
                    )
                    .await;
                    warn!(
                        "Binance worker exited; reconnecting after {:?}",
                        reconnect_delay
                    );
                }
                Err(e) => {
                    error!("Failed to connect to Binance websocket: {}", e);
                }
            }

            tokio::time::sleep(reconnect_delay).await;
        }
    });

    info!("Starting websocket server on {}", server_addr);
    warp::serve(routes).run(server_addr).await;
}

fn merge_generated_pair_rules(
    existing: &mut HashMap<String, config::PairRuleConfig>,
    generated: HashMap<String, config::PairRuleConfig>,
) {
    for (pair, generated_rule) in generated {
        existing
            .entry(pair)
            .and_modify(|current| {
                if current.min_notional.is_none() {
                    current.min_notional = generated_rule.min_notional;
                }
                if current.min_qty.is_none() {
                    current.min_qty = generated_rule.min_qty;
                }
                if current.qty_step.is_none() {
                    current.qty_step = generated_rule.qty_step;
                }
                if current.fee_bps.is_none() {
                    current.fee_bps = generated_rule.fee_bps;
                }
            })
            .or_insert(generated_rule);
    }
}

fn with_clients(clients: Clients) -> impl Filter<Extract = (Clients,), Error = Infallible> + Clone {
    warp::any().map(move || clients.clone())
}
