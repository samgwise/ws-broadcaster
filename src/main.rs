// A minimalistic server for handling pub-sub style message syndication over the internet.
// Built on Axum web and tungstenite for the the WS protocol.

mod messaging;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tracing::{event, Level};

use std::{net::SocketAddr, path::PathBuf};

use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use futures::{sink::SinkExt, stream::StreamExt};

use tokio::{
    sync::mpsc,
    time::{Duration, sleep}
};

#[derive(Clone)]
enum PubSubAction {
    Subscribe(mpsc::Sender<Message>),
    Publish(Message),
    Unsubscribe,
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

    // Keep a list of subscribers
    let mut subscribers = Vec::<mpsc::Sender<Message>>::new();

    let (manage_tx, mut manage_rx) = mpsc::channel(128);
    // Handle PubSubActions
    tokio::spawn(async move {
        loop {
            if let Some(msg) = manage_rx.recv().await {
                match msg {
                    PubSubAction::Subscribe(tx) => {
                        subscribers.push(tx)
                    }
                    PubSubAction::Publish(msg) => {
                        for sub in subscribers.as_slice() {
                            let _ = sub.send(msg.clone()).await;
                        }
                    }
                    PubSubAction::Unsubscribe => {
                        // Trigger a cleaning pass of the subscriber channels
                        subscribers = Vec::from_iter(
                            subscribers.into_iter().filter(|con| !con.is_closed()),
                        )
                    }
                }
            }
        }
    });

     // Ping clients to keep connections alive
     let publish_ping = manage_tx.clone();
     tokio::spawn(async move {
        let ping = PubSubAction::Publish(Message::Ping("!".into()));
         loop {
             sleep(Duration::from_secs(30)).await;
             let _ = publish_ping.send(ping.clone()).await;
         }
     });

    let asset_dir = PathBuf::from("./").join("assets");

    let post_publish_tx = manage_tx.clone();
    let app = Router::new()
        .fallback_service(ServeDir::new(asset_dir).append_index_html_on_directories(true))
        .route(
            "/ws",
            get(move |req| {
                let manage = manage_tx.clone();
                ws_handler(req, manage)
            }),
        )
        .route(
            "/publish",
            post(move |req| {
                let publish = post_publish_tx.clone();
                publish_handler(req, publish)
            }),
        )
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();

    tracing::debug!("listening on {}", listener.local_addr().unwrap());

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    manage: mpsc::Sender<PubSubAction>,
) -> impl IntoResponse {
    // Client connected!
    // TODO: Add info log here

    ws.on_upgrade(move |socket| handle_socket(socket, manage))
}

/// WebSocket state machine closure
async fn handle_socket(
    socket: WebSocket,
    manage: mpsc::Sender<PubSubAction>,
) {
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
                _ => {
                    let _ = manage.send(PubSubAction::Publish(msg)).await;
                }
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

async fn publish_handler(req: String, publish: mpsc::Sender<PubSubAction>) -> impl IntoResponse {

    let msg = Message::from(req);

    match publish.send(PubSubAction::Publish(msg)).await {
        Ok(_) => (StatusCode::OK, "").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Unable to handle messages at this time.").into_response()
    }
}
