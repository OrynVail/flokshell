use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

// Large enough for j/clients with a few hundred windows; a reply this size is
// already a compositor in trouble.
const REPLY_LIMIT: u64 = 16 << 20;

#[derive(Clone, Debug)]
pub struct Instance {
    dir: PathBuf,
}

fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/run/user/1000"))
}

// The unit's HYPRLAND_INSTANCE_SIGNATURE goes stale the moment Hyprland
// restarts, so it is only the first guess. Whichever instance answers on its
// event socket is the live one; newest directory first after that.
pub async fn discover() -> Option<Instance> {
    let root = runtime_dir().join("hypr");
    let mut candidates = Vec::new();
    if let Ok(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        candidates.push(root.join(sig));
    }
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    candidates.extend(found.into_iter().map(|(_, p)| p));

    for dir in candidates {
        if UnixStream::connect(dir.join(".socket2.sock")).await.is_ok() {
            return Some(Instance { dir });
        }
    }
    None
}

impl Instance {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub async fn request(&self, cmd: &str) -> Result<String> {
        let mut stream = UnixStream::connect(self.dir.join(".socket.sock"))
            .await
            .context("request socket")?;
        stream.write_all(cmd.as_bytes()).await?;
        let mut reply = String::new();
        (&mut stream)
            .take(REPLY_LIMIT)
            .read_to_string(&mut reply)
            .await?;
        Ok(reply)
    }

    pub async fn json<T: serde::de::DeserializeOwned>(&self, what: &str) -> Result<T> {
        let reply = self.request(&format!("j/{what}")).await?;
        serde_json::from_str(&reply).with_context(|| format!("j/{what}"))
    }

    // Hyprland answers "ok" or a message; anything but "ok" is a refusal.
    pub async fn dispatch(&self, arg: &str) -> Result<()> {
        let reply = self.request(&format!("dispatch {arg}")).await?;
        let reply = reply.trim();
        if reply != "ok" {
            bail!("{reply}");
        }
        Ok(())
    }

    // Runtime config for a Lua Hyprland: anything but "ok" names what it refused.
    pub async fn eval(&self, lua: &str) -> Result<()> {
        let reply = self.request(&format!("eval {lua}")).await?;
        let reply = reply.trim();
        if reply != "ok" {
            bail!("{reply}");
        }
        Ok(())
    }

    // The legacy keyword grammar, alone or as a [[BATCH]]; each line answers
    // for itself, so any one refusal fails it.
    pub async fn keyword(&self, cmd: &str) -> Result<()> {
        let reply = self.request(cmd).await?;
        if let Some(bad) = reply
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && *l != "ok")
        {
            bail!("{bad}");
        }
        Ok(())
    }

    pub async fn events(&self) -> Result<UnixStream> {
        UnixStream::connect(self.dir.join(".socket2.sock"))
            .await
            .context("event socket")
    }
}

// Lua dispatchers replaced the legacy grammar in 0.55; from then on every
// dispatch string is evaluated as Lua.
pub fn speaks_lua(version: &serde_json::Value) -> bool {
    let raw = version["version"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| version["tag"].as_str())
        .unwrap_or("");
    let mut parts = raw
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .map(|p| p.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), Some(minor)) => (major, minor) >= (0, 55),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lua_from_0_55() {
        assert!(speaks_lua(&json!({"version": "0.56.2"})));
        assert!(speaks_lua(&json!({"version": "0.55.0"})));
        assert!(speaks_lua(&json!({"version": "1.0.0"})));
        assert!(!speaks_lua(&json!({"version": "0.54.3"})));
        assert!(!speaks_lua(
            &json!({"version": "", "tag": "v0.49.0-12-gabc"})
        ));
        assert!(speaks_lua(&json!({"tag": "v0.56.2-b"})));
    }

    #[test]
    fn unreadable_version_assumes_current() {
        assert!(speaks_lua(&json!({})));
        assert!(speaks_lua(&json!({"version": "git"})));
    }
}
