use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::bus::{Bus, Request};
use crate::wayland::{self, Note};

const SERVICE: &str = "nightlight";
const KELVIN: u32 = 4500;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct State {
    active: bool,
    // False when every output's gamma is held by another client.
    available: bool,
}

#[derive(Deserialize)]
struct Set {
    on: bool,
}

pub struct Service {
    bus: Bus,
    wayland: wayland::Handle,
    notes: broadcast::Receiver<Note>,
    wanted: bool,
    state: State,
}

impl Service {
    pub fn new(bus: Bus, wayland: wayland::Handle, notes: broadcast::Receiver<Note>) -> Self {
        Self {
            bus,
            wayland,
            notes,
            wanted: false,
            state: State {
                active: false,
                available: true,
            },
        }
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        self.publish();
        loop {
            tokio::select! {
                Some(req) = requests.recv() => {
                    match (req.cmd.as_str(), serde_json::from_value::<Set>(req.args)) {
                        ("set", Ok(Set { on })) => {
                            self.wanted = on;
                            self.wayland.send(wayland::Request::Gamma(on.then_some(KELVIN)));
                        }
                        (cmd, _) => tracing::warn!("bad command: {cmd}"),
                    }
                }
                note = self.notes.recv() => match note {
                    Ok(Note::Gamma { applied, refused }) => {
                        self.state = State {
                            active: self.wanted && applied > 0,
                            available: applied > 0 || refused == 0,
                        };
                        self.publish();
                    }
                    Ok(Note::Connected(false)) => {
                        self.state.active = false;
                        self.publish();
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    }

    fn publish(&self) {
        match serde_json::to_value(&self.state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialise failed: {e}"),
        }
    }
}
