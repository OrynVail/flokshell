use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<&'a Value>,
}

// What a client asked of one service. `args` is that service's to interpret, so
// a tier that needs arguments never has to touch dispatch.
#[derive(Deserialize, Debug)]
pub struct Request {
    pub cmd: String,
    #[serde(default)]
    pub args: Value,
}

// One item of a keyed service. Keyed services exist for state too large to
// resend whole on every change: each item is replaced on its own, still
// idempotently, and a burst ends with `commit` so a client applies it at once.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Upsert(String, Value),
    Remove(String),
}

#[derive(Clone)]
pub struct Bus {
    inner: Arc<Mutex<Latest>>,
    tx: broadcast::Sender<String>,
}

#[derive(Default)]
struct Latest {
    state: HashMap<String, String>,
    keyed: HashMap<String, BTreeMap<String, String>>,
}

fn line(service: &str, event: &str, key: Option<&str>, data: Option<&Value>) -> Option<String> {
    match serde_json::to_string(&Event {
        service,
        event,
        key,
        data,
    }) {
        Ok(line) => Some(line),
        Err(e) => {
            tracing::error!(service, "serialise failed: {e}");
            None
        }
    }
}

impl Bus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Mutex::new(Latest::default())),
            tx,
        }
    }

    pub fn publish(&self, service: &str, data: &Value) {
        let Some(line) = line(service, "state", None, Some(data)) else {
            return;
        };
        let mut inner = self.inner.lock().unwrap();
        inner.state.insert(service.to_string(), line.clone());
        let _ = self.tx.send(line);
    }

    // The whole burst is one broadcast message, cached under the same lock, so
    // a client connecting mid-burst sees it entirely in its snapshot or
    // entirely in its stream, never half of each.
    pub fn apply(&self, service: &str, changes: &[Change]) {
        if changes.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        let items = inner.keyed.entry(service.to_string()).or_default();
        let mut burst = Vec::with_capacity(changes.len() + 1);
        for change in changes {
            match change {
                Change::Upsert(key, data) => {
                    let Some(l) = line(service, "upsert", Some(key), Some(data)) else {
                        continue;
                    };
                    items.insert(key.clone(), l.clone());
                    burst.push(l);
                }
                Change::Remove(key) => {
                    if items.remove(key).is_some() {
                        burst.extend(line(service, "remove", Some(key), None));
                    }
                }
            }
        }
        if burst.is_empty() {
            return;
        }
        burst.extend(line(service, "commit", None, None));
        let _ = self.tx.send(burst.join("\n"));
    }

    // An answer to one request, for whoever asked: broadcast, never cached,
    // so a client connecting later does not receive it.
    pub fn event(&self, service: &str, event: &str, data: &Value) {
        if let Some(l) = line(service, event, None, Some(data)) {
            let _ = self.tx.send(l);
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    // A keyed service is sent as reset, every item, commit: the client drops
    // whatever it held, so an item removed while it lagged cannot linger.
    pub fn snapshot(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<String> = inner.state.values().cloned().collect();
        for (service, items) in &inner.keyed {
            out.extend(line(service, "reset", None, None));
            out.extend(items.values().cloned());
            out.extend(line(service, "commit", None, None));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn events(lines: &[String]) -> Vec<Value> {
        lines
            .iter()
            .flat_map(|l| l.split('\n'))
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn a_burst_is_one_message_ending_in_commit() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();
        bus.apply(
            "c",
            &[
                Change::Upsert("a".into(), json!(1)),
                Change::Upsert("b".into(), json!(2)),
            ],
        );
        let msg = rx.try_recv().unwrap();
        assert!(rx.try_recv().is_err());
        let ev = events(&[msg]);
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0]["event"], "upsert");
        assert_eq!(ev[0]["key"], "a");
        assert_eq!(ev[2], json!({"service": "c", "event": "commit"}));
    }

    #[test]
    fn removing_an_absent_key_sends_nothing() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();
        bus.apply("c", &[Change::Remove("ghost".into())]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn snapshot_resets_then_lists_what_survived() {
        let bus = Bus::new();
        bus.publish("weather", &json!({"t": 20}));
        bus.apply(
            "c",
            &[
                Change::Upsert("a".into(), json!(1)),
                Change::Upsert("b".into(), json!(2)),
            ],
        );
        bus.apply("c", &[Change::Remove("a".into())]);
        let ev = events(&bus.snapshot());
        let keyed: Vec<_> = ev.iter().filter(|e| e["service"] == "c").collect();
        assert_eq!(keyed.len(), 3);
        assert_eq!(keyed[0]["event"], "reset");
        assert_eq!(keyed[1]["key"], "b");
        assert_eq!(keyed[2]["event"], "commit");
        assert!(ev.iter().any(|e| e["service"] == "weather"));
    }
}
