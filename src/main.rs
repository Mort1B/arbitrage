use log::{debug, error, info, warn};
use std::{collections::HashMap, convert::Infallible, net::SocketAddr, sync::Arc};
use tokio::sync::{mpsc, RwLock};
use tokio::time::Duration;
use warp::{ws::Message, Filter, Rejection};

mod analyzer;
mod config;
mod handlers;
mod models;
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

    log4rs::init_file("log_config.yaml", Default::default()).expect("failed to initialize logging");
    let config_file = std::fs::File::open("config.yaml").expect("Could not open config.yaml");
    let app_config: config::AppConfig =
        serde_yaml::from_reader(config_file).expect("Could not parse config.yaml");
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

fn with_clients(clients: Clients) -> impl Filter<Extract = (Clients,), Error = Infallible> + Clone {
    warp::any().map(move || clients.clone())
}
