<<<<<<< HEAD
use crate::{Client, Clients};
use futures::{FutureExt, StreamExt};
use log::{error, info};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;
use warp::ws::{Message, WebSocket};

pub async fn client_connection(ws: WebSocket, clients: Clients) {
    info!("establishing client connection... {:?}", ws);

    // Split the websocket stream object into separete sink and stream objects. This allows us to
    // receive messages from the client and send messages to the client in separete areas of the code
    let (client_ws_sender, mut client_ws_rcv) = ws.split();
    let (client_sender, client_rcv) = mpsc::unbounded_channel();

    let client_rcv = UnboundedReceiverStream::new(client_rcv);

    // Spawn a new task that keeps the client_ws_sender stream open until the client has disconnected
    tokio::task::spawn(client_rcv.forward(client_ws_sender).map(|result| {
        if let Err(e) = result {
            error!("error sending websocket msg: {}", e);
        }
    }));

    let uuid = Uuid::new_v4().simple().to_string();

    let new_client = Client {
        client_id: uuid.clone(),
        sender: Some(client_sender),
    };

    clients.lock().await.insert(uuid.clone(), new_client);

    // Handles receiving messages from the client and runs until the client is disconnected. If a message is received
    // the code in the while loop will call client_msg to do further processing of the message
    while let Some(result) = client_ws_rcv.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                error!("error receiving message for id {}): {}", uuid.clone(), e);
                break;
            }
        };
        client_msg(&uuid, msg, &clients).await;
    }

    clients.lock().await.remove(&uuid);
    info!("{} disconnected", uuid);
}

// The code tries to conver msg to a sttring, and if successful respons to it if the message is "ping".
// To send the response, we first obtain a lock on the clients and then get the client by its client id.
// We then use the sender field to send a message to the connected client.
async fn client_msg(client_id: &str, msg: Message, clients: &Clients) {
    info!("received message from {}: {:?}", client_id, msg);

    let message = match msg.to_str() {
        Ok(v) => v,
        Err(_) => return,
    };

    //every third minute binance ws will send a ping frame, if the websocket server does not receive a pong frame back
    // within a 10 minute period , the connection will be disconnected
    if message == "ping" || message == "ping\n" {
        let locked = clients.lock().await;
        match locked.get(client_id) {
            Some(v) => {
                if let Some(sender) = &v.sender {
                    info!("sending pong");
                    let _ = sender.send(Ok(Message::text("pong")));
                }
            }
            None => (),
        }
    };
}
=======
use crate::{Client, Clients};
use futures::{FutureExt, StreamExt};
use log::{debug, error, info};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;
use warp::ws::{Message, WebSocket};

pub async fn client_connection(ws: WebSocket, clients: Clients) {
    info!("establishing websocket client connection");
    let (client_ws_sender, mut client_ws_rcv) = ws.split();
    let (client_sender, client_rcv) = mpsc::unbounded_channel();
    let client_rcv = UnboundedReceiverStream::new(client_rcv);
    tokio::task::spawn(client_rcv.forward(client_ws_sender).map(|result| {
        if let Err(e) = result {
            error!("error sending websocket message: {}", e);
        }
    }));
    let uuid = Uuid::new_v4().simple().to_string();
    let new_client = Client {
        client_id: uuid.clone(),
        sender: client_sender,
    };
    clients.lock().await.insert(uuid.clone(), new_client);
    while let Some(result) = client_ws_rcv.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                error!("error receiving message for id {}: {}", uuid, e);
                break;
            }
        };
        client_msg(&uuid, msg, &clients).await;
    }
    clients.lock().await.remove(&uuid);
    info!("{} disconnected", uuid);
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
        let locked = clients.lock().await;
        locked.get(client_id).map(|client| client.sender.clone())
    };

    if let Some(sender) = sender {
        debug!("sending pong to {}", client_id);
        let _ = sender.send(Ok(Message::text("pong")));
    }
}
>>>>>>> 8a83541 (test2)
