mod bus;
mod config;
mod dbus;
mod ddc;
mod services;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use bus::{Bus, Request};

#[derive(Deserialize)]
struct Command {
    service: String,
    #[serde(flatten)]
    request: Request,
}

// XDG_RUNTIME_DIR, not /tmp: the system clears it on logout, which removes the
// stale-socket failure rather than leaving it to be documented.
fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/run/user/1000"));
    dir.join("flokd.sock")
}

// Every service here waits on a socket, a bus or an HTTP response. A worker per
// core would be threads to idle, not work to spread; anything CPU-bound that
// shows up later belongs on spawn_blocking regardless of this choice.
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FLOKD_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .without_time() // journald stamps every line already
        .init();

    let bus = Bus::new();
    let mut commands: HashMap<&str, mpsc::Sender<Request>> = HashMap::new();

    let (weather_tx, weather_rx) = mpsc::channel(1);
    commands.insert("weather", weather_tx);
    tokio::spawn(
        services::weather::Service::new(bus.clone())
            .context("weather service")?
            .run(weather_rx),
    );

    // Unlike a refresh, a dropped connect is lost.
    let (network_tx, network_rx) = mpsc::channel(8);
    commands.insert("network", network_tx);
    tokio::spawn(services::network::Service::new(bus.clone()).run(network_rx));

    let (bluetooth_tx, bluetooth_rx) = mpsc::channel(8);
    commands.insert("bluetooth", bluetooth_tx);
    tokio::spawn(services::bluetooth::Service::new(bus.clone()).run(bluetooth_rx));

    // A stop queued behind a watch must not be the one dropped.
    let (resources_tx, resources_rx) = mpsc::channel(4);
    commands.insert("resources", resources_tx);
    tokio::spawn(services::resources::Service::new(bus.clone()).run(resources_rx));

    // Room for a whole drag: a dropped set could be its last value.
    let (brightness_tx, brightness_rx) = mpsc::channel(64);
    commands.insert("brightness", brightness_tx);
    tokio::spawn(services::brightness::Service::new(bus.clone()).run(brightness_rx));

    let path = socket_path();
    // Single instance is the unit's job; a leftover socket is never a live peer.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    tracing::info!(socket = %path.display(), "listening");

    let accept = async {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let bus = bus.clone();
                    let commands = commands.clone();
                    tokio::spawn(async move {
                        if let Err(e) = serve(stream, bus, commands).await {
                            tracing::debug!("client gone: {e}");
                        }
                    });
                }
                Err(e) => tracing::warn!("accept failed: {e}"),
            }
        }
    };

    tokio::select! {
        _ = accept => {}
        _ = tokio::signal::ctrl_c() => tracing::info!("shutting down"),
    }

    let _ = std::fs::remove_file(&path);
    Ok(())
}

async fn serve(
    stream: UnixStream,
    bus: Bus,
    commands: HashMap<&'static str, mpsc::Sender<Request>>,
) -> Result<()> {
    let (read, mut write) = stream.into_split();

    // Subscribe before sending the snapshot. Events are idempotent full-state
    // replaces, so overlapping the two costs a duplicate line and closes the
    // window where an update could land between them and be lost.
    let mut events = bus.subscribe();
    for line in bus.snapshot() {
        write.write_all(line.as_bytes()).await?;
        write.write_all(b"\n").await?;
    }
    write.flush().await?;

    let mut lines = BufReader::new(read).lines();

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(line) => {
                    write.write_all(line.as_bytes()).await?;
                    write.write_all(b"\n").await?;
                    write.flush().await?;
                }
                // A client too slow to keep up gets the current state, not the
                // backlog it missed.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "client lagged");
                    for line in bus.snapshot() {
                        write.write_all(line.as_bytes()).await?;
                        write.write_all(b"\n").await?;
                    }
                    write.flush().await?;
                }
                Err(_) => break,
            },
            line = lines.next_line() => match line? {
                Some(line) if !line.trim().is_empty() => dispatch(&line, &commands),
                Some(_) => {}
                None => break,
            },
        }
    }

    Ok(())
}

fn dispatch(line: &str, commands: &HashMap<&'static str, mpsc::Sender<Request>>) {
    let Ok(c) = serde_json::from_str::<Command>(line) else {
        tracing::warn!("unparseable command: {line}");
        return;
    };
    let Some(tx) = commands.get(c.service.as_str()) else {
        tracing::warn!("unknown service: {}", c.service);
        return;
    };
    match tx.try_send(c.request) {
        Ok(()) => {}
        Err(TrySendError::Full(r)) => {
            tracing::debug!(service = %c.service, cmd = %r.cmd, "queue full, dropped")
        }
        Err(TrySendError::Closed(_)) => {
            tracing::error!(service = %c.service, "service has stopped")
        }
    }
}
