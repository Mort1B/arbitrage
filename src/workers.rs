<<<<<<< HEAD
use crate::{
    config::AppConfig,
    models::{self, DepthStreamWrapper},
    Clients,
};
use log::{debug, error, info};
use std::{collections::HashMap, net::TcpStream};
use tokio::time::{Duration, Instant};

use tungstenite::{protocol::WebSocket, stream::MaybeTlsStream};

// Here we have an infinate loop that "gather data" for our crypto triangle arbitrage and send it to connected clients every 0.5 seconds

pub async fn main_worker(
    clients: Clients,
    config: AppConfig,
    mut socket: WebSocket<MaybeTlsStream<TcpStream>>,
) {
    let mut pairs_data: HashMap<String, DepthStreamWrapper> = HashMap::new();
    let mut interval_timer = Instant::now();
    loop {
        // Not necessary
        // let connected_client_count = clients.lock().await.len();
        // if connected_client_count == 0 {
        //     tokio::time::sleep(Duration::from_millis(500)).await;
        //     debug!("No clients connected, skip sending data");
        // }

        let msg = socket.read_message().expect("Error reading message");
        let msg = match msg {
            tungstenite::Message::Text(s) => s,
            tungstenite::Message::Ping(p) => {
                info!("Ping message received! {:?}", p);
                continue;
            }
            tungstenite::Message::Pong(p) => {
                info!("Pong received: {:?}", p);
                continue;
            }
            _ => {
                error!("Error getting text: {:?}", msg);
                continue;
            }
        };

        info!("msg: {}", msg);
        let parsed: models::DepthStreamWrapper = serde_json::from_str(&msg).expect("Can't parse");

        let pair_key = parsed.stream.split_once('@').unwrap().0;
        pairs_data.insert(pair_key.to_string(), parsed);

        if interval_timer.elapsed().as_millis() < 105 {
            continue;
        }
        let data_copy = pairs_data.clone();
        let triangles = config.triangles.to_vec();
        let cclients = clients.clone();
        tokio::task::spawn(async move {
            for triangle_config in triangles.iter() {
                process_triangle_data(
                    &data_copy,
                    &triangle_config.pairs[0],
                    &triangle_config.pairs[1],
                    &triangle_config.pairs[2],
                    [
                        &triangle_config.parts[0],
                        &triangle_config.parts[1],
                        &triangle_config.parts[2],
                    ],
                    cclients.clone(),
                )
                .await;
            }
        });

        interval_timer = Instant::now();
    }
}

// Here we write a new functio which takes a HashMap of DepthStreamWrapper and taked 3 coin pairs from it and uses that data to call calc_triangle_step
/*
pairs_data = A HashMap of ask and bid data for a particular pair of coins,
start_pair = the first pair of the triangle, eks: ethbtc
mid_pair = the second pair of the triangle, eks: bnbeth
end_pair = the last pair of the triangle, eks: bnbbtc
triangle = array of &str containing symbols that make up the triangle in order !!!
clients = list of connected clients to send the calculated profit data to.
*/
async fn process_triangle_data(
    pairs_data: &HashMap<String, DepthStreamWrapper>,
    start_pair: &str,
    mid_pair: &str,
    end_pair: &str,
    triangle: [&str; 3],
    clients: Clients,
) {
    info!(
        "processing triangle {:?}: {}->{}->{}",
        triangle, start_pair, mid_pair, end_pair
    );

    // Here we attempt to grap data from the HashMap for the pairs we are interested in and put them in a tuple
    let data = (
        pairs_data.get(start_pair),
        pairs_data.get(mid_pair),
        pairs_data.get(end_pair),
    );
    // Here we go into a match clause to see if each piece has a value, if not, we exit the function.
    // Otherwise we can continue
    let (start_pair_data, mid_pair_data, end_pair_data) = match data {
        (Some(s), Some(m), Some(e)) => (s, m, e),
        _ => {
            info!(
                "{:?} One or more of the pairs were not found, skipping",
                (start_pair, mid_pair, end_pair)
            );
            return;
        }
    };

    // In this part we create a vec to hold the profit values and then loop through the length of the ask list.
    // We then call calc_triangle_step three times, one for each triangle part. Starting with 1.0 for the amount
    // param and then using the result of the calculation as the amount for each following call of calc_triangle_step
    // Finally, we subtract 1 from the final value and the leftover value is the profit (in the coin we started with!!!!)
    let mut profits: Vec<f64> = Vec::new();

    for i in 0..start_pair_data.data.asks.len() {
        let mut triangle_profit = calc_triangle_step(
            1.0, //Amount of coin A
            start_pair_data.data.asks[i].price,
            start_pair_data.data.bids[i].price,
            start_pair,
            triangle[0],
        );
        triangle_profit = calc_triangle_step(
            triangle_profit,
            mid_pair_data.data.asks[i].price,
            mid_pair_data.data.bids[i].price,
            mid_pair,
            triangle[1],
        );
        triangle_profit = calc_triangle_step(
            triangle_profit,
            end_pair_data.data.asks[i].price,
            end_pair_data.data.bids[i].price,
            end_pair,
            triangle[2],
        );

        let norm_profit = triangle_profit - 1.0;
        profits.push(norm_profit);
        if norm_profit > 0.0 {
            info!(target: "profit", "{:?} positive profit: {:.5}% ({} {})", triangle, (norm_profit*100.0), norm_profit, triangle[0]);
        }
    }

    info!("{:?} potential profits: {:?}", triangle, profits);

    // Now we can send the data to the clients, continuing with the code in the process_triangle_data function in workers.rs
    // Here we simply fill in the fields of the triangleAritrageData object and then use the familiar method of looping through clients and calling send on the
    // sender object to send the data to the connected clients
    let triangle_data = models::TriangleArbitrageData {
        start_pair_data: start_pair_data.clone(),
        mid_pair_data: mid_pair_data.clone(),
        end_pair_data: end_pair_data.clone(),
        profits,
        triangle: [
            triangle[0].to_string(),
            triangle[1].to_string(),
            triangle[2].to_string(),
        ],
    };
    clients.lock().await.iter().for_each(|(_, client)| {
        if let Some(sender) = &client.sender {
            let _ = sender.send(Ok(warp::ws::Message::text(
                serde_json::to_string(&triangle_data).unwrap(),
            )));
        }
    });
}

// trade_amount = how much of a coin we are trading for the target
// ask_price = the ask price for tha pairing.
// bid_price = the bid price for the pairing
// pair_name = the name of the pair as it is known in the binance API, eks: ethbtc.
// triangle_part = the part othe the triangle we are currenctly concidering, what coin are are trading fot the other pair.
fn calc_triangle_step(
    trade_amount: f64,
    ask_price: f64,
    bid_price: f64,
    pair_name: &str,
    triangle_part: &str,
) -> f64 {
    // subtract trading fee - should maybe look at pairs with no fees now !??

    let trade_amount = match pair_name {
        "btcusdt" | "btcbusd" | "btcusdc" => trade_amount,
        _ => trade_amount - ((trade_amount / 100.0) * 0.075),
    };

    // let trade_amount = trade_amount - ((trade_amount / 100.0) * 0.075); // old solution
    // Compare first part of the part to the part of the triangle
    // to determine on what side of the trade we should be
    if pair_name[..triangle_part.len()] == *triangle_part {
        // sell side
        trade_amount * bid_price
    } else {
        // buy side
        trade_amount / ask_price
    }
}

//Let’s use the triangle arbitrage scenario to look at how this function is used. We start with the data on the ETH-BTC pair and go through there.
// We will use 1 BTC to buy ETH, let’s say that the ask price is 0.062 and the buy price 0.061.
// The parameters will look like this: calc_triangle_step(1, 0.062, 0.061, "ethbtc", "btc").
// On line 178 “btc” is not the first part of the pair, so we are on the buy side of the trade. Using BTC to buy ETH.
// The amount of ETH we get for 1 BTC: 1 / 0.062 = ~16.13
// Next we use 16.13 ETH to buy BNB, let’s say the ask price is 0.1313 and the bid price 0.1312.
// The parameters will look like this: calc_triangle_step(16.13, 0.1313, 0.1312, "bnbeth", "eth").
// On line 178 “eth” is not the first part of the pair, so we are on the buy side of the trade. Using ETH to buy BNB.
// The amount of BNB we get for 16.13 ETH: 16.13 / 0.1313 =~122.84
// Finally we use the 122.84 BNB to buy BTC, let’s say the ask price is 0.008185 and the bid price is 0.008184.
// The parameters will look like this: calc_triangle_step(122.84, 0.008185, 0.008184, "bnbbtc", "bnb").
// On line 178 “bnb” is the first part of the pair, so we are on the sell side of the trade. Using BNB to sell for BTC.
// The amount of BTC we get for 122.84 BNB: 122.84 * 0.008184 =~1.0053.
// The example trades would yield 0.0053 BTC profit. That is about 0.5% profit. Not bad
=======
use crate::{models, Clients};
use log::{debug, error, info, warn};
use std::net::TcpStream;
use tungstenite::{stream::MaybeTlsStream, WebSocket};
use warp::ws::Message;

type BinanceSocket = WebSocket<MaybeTlsStream<TcpStream>>;

pub async fn main_worker(clients: Clients, mut socket: BinanceSocket) {
    loop {
        let msg = match socket.read_message() {
            Ok(msg) => msg,
            Err(e) => {
                error!("error reading Binance message: {}", e);
                break;
            }
        };

        let msg = match msg {
            tungstenite::Message::Text(s) => s,
            tungstenite::Message::Close(frame) => {
                info!("Binance websocket closed: {:?}", frame);
                break;
            }
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => continue,
            _ => {
                debug!("skipping non-text Binance frame");
                continue;
            }
        };

        let parsed: models::DepthStreamWrapper = match serde_json::from_str(&msg) {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!("failed to parse Binance depth payload: {}", e);
                continue;
            }
        };

        debug!(
            "received {} with {} bids and {} asks",
            parsed.stream,
            parsed.data.bids.len(),
            parsed.data.asks.len()
        );

        let payload = match serde_json::to_string(&parsed) {
            Ok(payload) => payload,
            Err(e) => {
                warn!("failed to serialize payload for clients: {}", e);
                continue;
            }
        };

        let senders = {
            let locked = clients.lock().await;
            if locked.is_empty() {
                Vec::new()
            } else {
                locked
                    .values()
                    .map(|client| client.sender.clone())
                    .collect::<Vec<_>>()
            }
        };

        if senders.is_empty() {
            continue;
        }

        debug!("broadcasting payload to {} client(s)", senders.len());
        for sender in senders {
            let _ = sender.send(Ok(Message::text(payload.clone())));
        }
    }
}
>>>>>>> 8a83541 (test2)
