use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::time::Instant;
use zbus::message::Type as MessageType;
use zbus::wire::{ObjectPath, OwnedObjectPath, OwnedVariant, Type};
use zbus::{Connection, MatchRule, Message, MessageStream};

pub type Props = HashMap<String, OwnedVariant>;

pub fn take<T>(props: &mut Props, key: &str) -> Option<T>
where
    T: TryFrom<OwnedVariant>,
{
    props.remove(key).and_then(|v| T::try_from(v).ok())
}

pub fn take_path(props: &mut Props, key: &str) -> String {
    take::<OwnedObjectPath>(props, key)
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into())
}

pub fn path(s: &str) -> Result<ObjectPath<'_>> {
    Ok(ObjectPath::try_from(s)?)
}

pub async fn call<B>(
    conn: &Connection,
    dest: &str,
    path: &str,
    iface: &str,
    method: &str,
    body: &B,
) -> Result<Message>
where
    B: Serialize + Type,
{
    conn.call_method(Some(dest), path, Some(iface), method, body)
        .await
        .with_context(|| format!("{iface}.{method} on {path}"))
}

pub async fn set<'a>(
    conn: &Connection,
    dest: &str,
    path: &str,
    iface: &str,
    prop: &str,
    value: impl Into<zbus::wire::Variant<'a>>,
) -> Result<()> {
    call(
        conn,
        dest,
        path,
        "org.freedesktop.DBus.Properties",
        "Set",
        &(iface, prop, value.into()),
    )
    .await?;
    Ok(())
}

// Bus-side matches outlive a service restart; this stream reports one.
pub async fn owner_changes(conn: &Connection, name: &str) -> Result<MessageStream> {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .arg(0, name)
        .build()?;
    Ok(MessageStream::for_match_rule(rule, conn, None).await?)
}

pub async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
