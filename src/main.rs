// A minimalistic server for handling pub-sub style message syndication over the internet.
// Built on Axum web and tungstenite for the the WS protocol.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade, CloseFrame},
        connect_info::ConnectInfo
    },
    response::IntoResponse,
    routing::get,
    Router
};
use axum_extra::{typed_header, TypedHeader};
use tracing::{event, Level};

use std::sync::Arc;
use std::{net::SocketAddr, path::PathBuf};

use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer}
};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use futures::{sink::SinkExt, stream::StreamExt};

use tokio::sync::{Mutex, mpsc};

enum PubSubAction {
    Subscribe(mpsc::Sender<Message>),
    Unsubscribe
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "example_websockets=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let (publish_tx, mut subscribe_rx) = mpsc::channel::<Message>(128);
    // let publish = Arc::new(Mutex::new(publish_tx));

    let subscribers = Arc::new(Mutex::new(Vec::<mpsc::Sender<Message>>::new()));
    let subs_list_ref = Arc::clone(&subscribers);

    // Publish to all subscribers
    tokio::spawn(async move {
        loop {
            if let Some(msg) = subscribe_rx.recv().await {
                let subs_list = subs_list_ref.lock().await;
                for s in subs_list.as_slice() {
                    let _ = s.send(msg.clone()).await;
                }
            }
        }
    });


    let (manage_tx, mut manage_rx) = mpsc::channel(128);
    let subs_list_ref = Arc::clone(&subscribers);
    // Handle adding and removing subscribers
    tokio::spawn(async move {
        loop {
            if let Some(msg) = manage_rx.recv().await {
                match msg {
                    PubSubAction::Subscribe(tx) => {
                        let mut subs_list = subs_list_ref.lock().await;
                        subs_list.push(tx)
                    },
                    PubSubAction::Unsubscribe => {
                        // Trigger a cleaning pass of the subscriber channels
                        let mut subs_list = subs_list_ref.lock().await;
                        *subs_list = Vec::from_iter(subs_list.clone().into_iter().filter(|con| !con.is_closed()))
                    }
                }
            }
        }
    });

    let asset_dir = PathBuf::from("./").join("assets");

    let app = Router::new()
        .fallback_service(ServeDir::new(asset_dir).append_index_html_on_directories(true))
        .route("/ws", get(move |req, info| {
            // let subscriber = subscribers.lock();
            
            // let subscriber_list = subscribers.lock().await;
            // subscribers.get_mut().push(inbox.clone());
            // let subs = subscribers.get_mut().push(inbox);

            let publish = publish_tx.clone();
            let manage = manage_tx.clone();
            ws_handler(req, publish, manage, info)
        }))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true))
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();

    tracing::debug!("listening on {}", listener.local_addr().unwrap());

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>()
    )
    .await.unwrap()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    publish: mpsc::Sender<Message>,
    manage: mpsc::Sender<PubSubAction>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>
) -> impl IntoResponse {

    // Client connected!
    // TODO: Add info log here

    ws.on_upgrade(move |socket| handle_socket(socket, addr, publish, manage))
}

/// WebSocket state machine closure
async fn handle_socket(mut socket: WebSocket, who: SocketAddr, publish: mpsc::Sender<Message>, manage: mpsc::Sender<PubSubAction>) {
   

    let (inbox_tx, mut inbox_rx) = mpsc::channel(32);
    let _ = manage.send(PubSubAction::Subscribe(inbox_tx)).await;

    let (mut sender, mut receiver) = socket.split();

    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = inbox_rx.recv().await {
            let _ = sender.send(msg).await;
        }
    });

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match &msg {
                Message::Close(_end) => {
                    // Cleanup
                    let _ = manage.send(PubSubAction::Unsubscribe).await;
                    break;
                }
                Message::Ping(_val) => {
                    // Do not propagate
                }
                Message::Pong(_val) => {
                    // Do not propagate
                }
                _ => { let _ = publish.send(msg).await; }
            }
        }
    });

    // If any task exits, abort execution of the other
    tokio::select! {
        rv_a = (&mut send_task) => {
            match rv_a {
                Ok(a) => a,
                Err(a) => event!(Level::ERROR, "Failed sending messages to client: {}", a)
            }
            recv_task.abort();
        },
        rv_b= (&mut recv_task) => {
            match rv_b {
                Ok(b) => b,
                Err(b) => event!(Level::ERROR, "Failed receiving messages from client: {}", b)
            }
            send_task.abort();
        }
    }

    event!(Level::DEBUG, "WebSocket finished");
}