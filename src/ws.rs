use crate::{Client, Clients, OutboundMessage};
use futures_util::StreamExt;
use log::{debug, error, info};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;
use warp::ws::{Message, WebSocket};

const CLIENT_CHANNEL_CAPACITY: usize = 128;

pub async fn client_connection(ws: WebSocket, clients: Clients) {
    info!("establishing websocket client connection");

    let (client_ws_sender, mut client_ws_rcv) = ws.split();
    let (client_sender, client_rcv) = mpsc::channel::<OutboundMessage>(CLIENT_CHANNEL_CAPACITY);
    let client_id = Uuid::new_v4().simple().to_string();

    tokio::spawn(async move {
        if let Err(e) = ReceiverStream::new(client_rcv)
            .forward(client_ws_sender)
            .await
        {
            error!("error sending websocket message: {}", e);
        }
    });

    clients.write().await.insert(
        client_id.clone(),
        Client {
            sender: client_sender,
        },
    );

    while let Some(result) = client_ws_rcv.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                error!("error receiving message for id {}: {}", client_id, e);
                break;
            }
        };

        client_msg(&client_id, msg, &clients).await;
    }

    clients.write().await.remove(&client_id);
    info!("{} disconnected", client_id);
}

async fn client_msg(client_id: &str, msg: Message, clients: &Clients) {
    debug!("received message from {}: {:?}", client_id, msg);

    let message = match msg.to_str() {
        Ok(v) => v,
        Err(_) => return,
    };

    if message.trim() != "ping" {
        return;
    }

    let sender = {
        let locked = clients.read().await;
        locked.get(client_id).map(|client| client.sender.clone())
    };

    if let Some(sender) = sender {
        debug!("sending pong to {}", client_id);
        let _ = sender.try_send(Ok(Message::text("pong")));
    }
}
