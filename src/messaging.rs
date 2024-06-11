use tokio::sync::{mpsc, oneshot};
use axum::extract::ws::Message;

// 1..^Inf
pub fn auto_inc_u16() -> impl FnMut() -> u16 {
    let mut counter = 0u16;
    move || {
        counter += 1;
        counter
    }
}

/// Reply to requests for an u16 id
pub async fn id_generator(mut rx: mpsc::Receiver<oneshot::Sender<u16>>) {
    let mut auto_inc = auto_inc_u16();
    while let Some(id_request) = rx.recv().await {
        let _ = id_request.send(auto_inc());
    }
}

pub fn client_id_as_hexcode(id: u16) -> String {
    format!("{:X}", id)
}

pub fn client_id_from_hexcode(id_text: &String) -> Result<u16, String> {
    match u16::from_str_radix(&id_text, 16) {
        Ok(id) => Ok(id),
        Err(err) => Err(format!("Failed extracting id from Hex string {} with error {:?}", id_text, err))
    }
}

/// PubSub Messaging wrapper
#[derive(Clone)]
pub struct PubSubMessage {
    pub message: Message,
    pub id_origin: u16,
    pub name_space: String
}

impl PubSubMessage {
    pub fn new_server_message(name_space: &str, message: Message) -> Self {
        PubSubMessage {message, id_origin: 0u16, name_space: name_space.to_string()}
    }

    pub fn new_client_message(name_space: &str, id_origin: u16, message: Message) -> Self {
        PubSubMessage {message, id_origin, name_space: name_space.to_string()}
    }

    pub fn new_ping(name_space: &str) -> Self {
        PubSubMessage {message: Message::Ping("!".into()), id_origin: 0u16, name_space: name_space.to_string()}
    }
}

pub trait RouteNameSpace {
    fn is_in_scope(&self, message_ns: &String) -> bool;

    fn is_origin(&self, id: u16) -> bool;
}

#[derive(Clone)]
pub struct PubSubClient {
    pub tx: mpsc::Sender<PubSubMessage>,
    id: u16,
    name_space: String
}

impl PubSubClient {
    pub fn new(id: u16, tx: mpsc::Sender<PubSubMessage>, name_space: String) -> PubSubClient {
        let name_space = if name_space.ends_with("/") {
            name_space
        }
        else {
            format!("{}/", name_space)
        };

        PubSubClient {tx, id, name_space}
    }

    fn unique_ns(&self) -> String {
        format!("{}{}", self.name_space, self.id)
    }
}

impl RouteNameSpace for PubSubClient {
    fn is_in_scope(&self, message_ns: &String) -> bool {
        let unique_ns = self.unique_ns();// = message.name_space.clone();
        unique_ns == *message_ns || self.name_space.contains(message_ns)
    }

    fn is_origin(&self, id: u16) -> bool {
        self.id == id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_scope() {
        let (tx, _) = mpsc::channel(1);
        let mut id_source = auto_inc_u16();
        let id = id_source();
        let client = PubSubClient::new(id, tx, "/hello/world".into());
        // match
        assert_eq!(client.is_in_scope(&"/hello/world".into()), true);
        assert_eq!(client.is_in_scope(&"/hello/world/".into()), true);
        assert_eq!(client.is_in_scope(&"/hello".into()), true);
        assert_eq!(client.is_in_scope(&"/".into()), true);
        assert_eq!(client.is_in_scope(&"".into()), true);

        // No match
        assert_eq!(client.is_in_scope(&"/hello/world/and/mars".into()), false);
        assert_eq!(client.is_in_scope(&"/hello/world-wide-web".into()), false);
        // N.B. the name '/hello/worl' will currently match, but it is not to specification
        // This isn't an issue in practice, since Client namespaces are also the source of 
        // Message namespaces and client namespaces are sanitised to end in a '/'.
        assert_eq!(client.is_in_scope(&"/hello/worl/".into()), false);
        assert_eq!(client.is_in_scope(&"/Hello/World".into()), false);
        assert_eq!(client.is_in_scope(&"/hi/mars".into()), false);

        // Unique name tests
        assert_eq!(client.unique_ns(), "/hello/world/".to_owned() + &client_id_as_hexcode(client.id))
    }

    #[test]
    fn is_client_origin_of_message() {
        let (tx, _) = mpsc::channel(1);
        let mut id_source = auto_inc_u16();
        let id = id_source();
        let client = PubSubClient::new(id, tx, "/hello/world".into());

        let message_same_origin = client.id;
        assert_eq!(client.is_origin(message_same_origin), true);
        // assert_eq!(eq_origin(&client.id, &message_same_origin), true);

        let message_other_origin = id_source();
        assert_eq!(client.is_origin(message_other_origin), false);
    }

    #[test]
    fn id_to_text_roundtrip() {
        let mut id_source = auto_inc_u16();
        for _ in 1..512 {
            let id = id_source();
            let id_text = client_id_as_hexcode(id);
            let id_text_to_int = client_id_from_hexcode(&id_text);
            assert_eq!(id_text_to_int.is_ok(), true);
            if let Ok(id_from_text) = id_text_to_int {
                assert_eq!(id, id_from_text)
            }
        }
    }
}