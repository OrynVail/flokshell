use std::path::PathBuf;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

pub fn dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"));
    base.join("flokshell/config")
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".cache"));
    base.join("flokshell")
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

// A missing file is the normal case, not an error: the shell only writes a
// config once something in it has been changed from the compiled-in default.
pub async fn read(name: &str) -> serde_json::Value {
    match tokio::fs::read(dir().join(name)).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!(name, "malformed, using defaults: {e}");
            serde_json::Value::Null
        }),
        Err(_) => serde_json::Value::Null,
    }
}

/// Fires once per settled batch of writes in the config directory.
///
/// Watches the directory, never the file. The shell saves config atomically
/// (temp file + rename), which replaces the inode — a watch on the file itself
/// would survive exactly one save and then observe nothing forever.
pub fn watch() -> anyhow::Result<mpsc::Receiver<()>> {
    let (raw_tx, mut raw_rx) = mpsc::unbounded_channel();
    let (tx, rx) = mpsc::channel(1);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = raw_tx.send(());
        }
    })?;
    watcher.watch(&dir(), RecursiveMode::NonRecursive)?;

    tokio::spawn(async move {
        let _watcher = watcher; // dropping it stops delivery
        while raw_rx.recv().await.is_some() {
            // One rename lands as several events; wait for quiet before reporting.
            while tokio::time::timeout(Duration::from_millis(200), raw_rx.recv())
                .await
                .is_ok()
            {}
            if tx.send(()).await.is_err() {
                break;
            }
        }
    });

    Ok(rx)
}
