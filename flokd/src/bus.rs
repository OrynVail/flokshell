use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

// Every event is a full-state replace for one service, so a client may receive
// the same state twice without consequence. That is what lets a new connection
// subscribe first and request the snapshot second, with no gap and no lock held
// across the two.
#[derive(Serialize)]
pub struct Event<'a> {
    pub service: &'a str,
    pub event: &'a str,
    pub data: &'a Value,
}

#[derive(Clone)]
pub struct Bus {
    latest: Arc<Mutex<HashMap<String, String>>>,
    tx: broadcast::Sender<String>,
}

impl Bus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            latest: Arc::new(Mutex::new(HashMap::new())),
            tx,
        }
    }

    pub fn publish(&self, service: &str, data: &Value) {
        let line = match serde_json::to_string(&Event {
            service,
            event: "state",
            data,
        }) {
            Ok(line) => line,
            Err(e) => {
                tracing::error!(service, "serialise failed: {e}");
                return;
            }
        };

        self.latest
            .lock()
            .unwrap()
            .insert(service.to_string(), line.clone());
        let _ = self.tx.send(line);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.latest.lock().unwrap().values().cloned().collect()
    }
}
