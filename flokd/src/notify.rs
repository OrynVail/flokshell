use std::collections::HashMap;

use anyhow::Result;
use futures_util::StreamExt;
use zbus::Variant;
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, MessageStream};

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub struct Notice<'a> {
    pub summary: &'a str,
    pub body: &'a str,
    pub icon: &'a str,
    pub urgency: u8,
    /// Alternating key and label, as the spec lays them out.
    pub actions: &'a [&'a str],
}

impl Default for Notice<'_> {
    fn default() -> Self {
        Self {
            summary: "",
            body: "",
            icon: "",
            urgency: 1,
            actions: &[],
        }
    }
}

pub async fn send(conn: &Connection, n: Notice<'_>) -> Result<u32> {
    let mut hints: HashMap<&str, Variant> = HashMap::new();
    hints.insert("urgency", Variant::U8(n.urgency));
    let reply = conn
        .call_method(
            Some(DEST),
            PATH,
            Some(DEST),
            "Notify",
            &(
                "flokshell",
                0u32,
                n.icon,
                n.summary,
                n.body,
                n.actions,
                hints,
                -1i32,
            ),
        )
        .await?;
    Ok(reply.body().deserialize()?)
}

/// (notification id, action key) for every action clicked, from any sender.
pub async fn actions(conn: &Connection) -> Result<impl futures_util::Stream<Item = (u32, String)>> {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(DEST)
        .member("ActionInvoked")
        .build()?;
    Ok(MessageStream::for_match_rule(rule, conn, None)
        .await?
        .filter_map(|m| async move { m.ok()?.body().deserialize::<(u32, String)>().ok() }))
}
