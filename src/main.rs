// A minimalistic server for handling pub-sub style message syndication over the internet.
// Built on Axum web and tungstenite for the the WS protocol.

mod messaging;
use messaging::*;

use axum::{
    extract::{
        Query,
        ws::{Message, WebSocket, WebSocketUpgrade}
    },
    http::{
        StatusCode,
        header::HeaderMap,
        header::HeaderValue
    },
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tracing::{event, Level};

use std::{net::SocketAddr, path::PathBuf};
use std::collections::HashMap;

use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use futures::{sink::SinkExt, stream::StreamExt};

use tokio::{
    sync::{mpsc, oneshot},
    time::{Duration, sleep}
};

#[derive(Clone)]
enum PubSubAction {
    Subscribe(PubSubClient),
    Publish(PubSubMessage),
    Unsubscribe,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "publish_handler=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Id generator
    let (id_tx, id_rx) = mpsc::channel::<oneshot::Sender<u16>>(100);
    tokio::spawn(id_generator(id_rx));

    // Keep a list of subscribers
    let mut subscribers = Vec::<PubSubClient>::new();

    let (manage_tx, mut manage_rx) = mpsc::channel(128);
    // Handle PubSubActions
    tokio::spawn(async move {
        loop {
            if let Some(msg) = manage_rx.recv().await {
                match msg {
                    PubSubAction::Subscribe(client) => {
                        subscribers.push(client)
                    }
                    PubSubAction::Publish(msg) => {
                        for sub in subscribers.as_slice() {
                            let _ = sub.tx.send(msg.clone()).await;
                        }
                    }
                    PubSubAction::Unsubscribe => {
                        // Trigger a cleaning pass of the subscriber channels
                        subscribers = Vec::from_iter(
                            subscribers.into_iter().filter(|con| !con.tx.is_closed()),
                        )
                    }
                }
            }
        }
    });

     // Ping clients to keep connections alive
     let publish_ping = manage_tx.clone();
     tokio::spawn(async move {
        let ping = PubSubAction::Publish(PubSubMessage::new_ping("/"));
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
            get(move |path, req| {
                let manage = manage_tx.clone();
                let id_source = id_tx.clone();
                
                ws_handler(path, req, id_source, manage)
            }),
        )
        .route(
            "/publish",
            post(move |header, req| {
                let publish = post_publish_tx.clone();
                publish_handler(header, req, publish)
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
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
    id_source: mpsc::Sender<oneshot::Sender<u16>>,
    manage: mpsc::Sender<PubSubAction>,
) -> impl IntoResponse {
    // Client connected!
    // TODO: Add info log here
    let (id_request, id_response) = oneshot::channel();
    let _ = id_source.send(id_request).await;

    match id_response.await {
        Ok(id) => {
            let mut namespace = String::from("/");
            if let Some(ns) = params.get("namespace") {
                namespace.clone_from(ns);
            }

            let header_namespace = namespace.clone();
            let mut ws_upgrade = ws.on_upgrade(move |socket| handle_socket(namespace, socket, id, manage));
            
            // Add subscriber id to header
            let header = ws_upgrade.headers_mut();
            header.append("X-Subscriber-Id", id.into());
            header.append(
                "X-Publish-Namespace", 
                HeaderValue::from_str(&header_namespace)
                    .unwrap_or_else(|_| HeaderValue::from_static("/"))
                );

            ws_upgrade
        },
        Err(e) => {
            event!(Level::ERROR, "Failed obtaining id for client: {:?}", e);
            (StatusCode::SERVICE_UNAVAILABLE, "Unable to handle subscriptions at this time.").into_response()
        }
    }
}

/// WebSocket state machine closure
async fn handle_socket(
    namespace: String,
    socket: WebSocket,
    id: u16,
    manage: mpsc::Sender<PubSubAction>,
) {
   
    // TX/RX pair for the subscription
    let (inbox_tx, mut inbox_rx) = mpsc::channel(32);
    let message_namespace = namespace.clone();

    let client = PubSubClient::new(id, inbox_tx, namespace.into());
    let _ = manage.send(PubSubAction::Subscribe(client.clone())).await;

    let (mut sender, mut receiver) = socket.split();

    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = inbox_rx.recv().await {
            if client.accept_message(msg.id_origin, &msg.namespace) {
                let _ = sender.send(msg.message).await;
            }
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
                    let msg = PubSubMessage::new_client_message(&message_namespace, id, msg);
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

async fn publish_handler(headers: HeaderMap, req: String, publish: mpsc::Sender<PubSubAction>) -> impl IntoResponse {
    // Get name space from the header if provided
    let namespace = match headers.get("X-Publish-Namespace") {
        Some(ns) => ns.to_str().unwrap_or_default(),
        None => "/"
    };

    let msg = PubSubMessage::new_server_message(&namespace, Message::from(req));

    match publish.send(PubSubAction::Publish(msg)).await {
        Ok(_) => (StatusCode::OK, "").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Unable to handle messages at this time.").into_response()
    }
    
}
