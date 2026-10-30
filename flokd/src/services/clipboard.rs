use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use percent_encoding::percent_decode_str;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use crate::bus::{Bus, Request};
use crate::config;
use crate::datacontrol::{self, Selection};

const SERVICE: &str = "clipboard";
const SCHEMA: &str = include_str!("clipboard.sql");
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const LIST_SIZE: i64 = 100;
const SEARCH_SIZE: i64 = 200;
const PREVIEW_CHARS: usize = 100;
// Enough to read in a panel; a copy always takes the whole item from the database.
const CONTENT_MAX: usize = 512 * 1024;
const URI: &str = "text/uri-list";

const COLUMNS: &str = "id, mime_type, preview, is_image, binary_path, content_hash, size, \
                       created_at, pinned, alias, display_index";
const ORDER: &str = "ORDER BY pinned DESC, display_index ASC, updated_at DESC, id DESC";

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    mime: String,
    preview: String,
    is_image: bool,
    is_file: bool,
    binary_path: String,
    hash: String,
    size: i64,
    created_at: i64,
    pinned: bool,
    alias: String,
    display_index: i64,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Copy { id: String },
    Text { text: String },
    File { path: String, mime: String },
    Content { id: String },
    Search { query: String },
    Delete { id: String },
    Clear,
    Pin { id: String },
    Alias { id: String, alias: String },
    Swap { a: String, b: String },
}

impl TryFrom<Request> for Command {
    type Error = serde_json::Error;

    fn try_from(req: Request) -> Result<Self, Self::Error> {
        let mut v = json!({ "cmd": req.cmd });
        if !req.args.is_null() {
            v["args"] = req.args;
        }
        serde_json::from_value(v)
    }
}

/// What a copy turns into, before it has a row.
#[derive(Debug, PartialEq)]
struct New<'a> {
    hash: String,
    mime: &'a str,
    preview: String,
    content: String,
    size: i64,
    blob: Option<&'a [u8]>,
}

pub struct Service {
    bus: Bus,
    clip: datacontrol::Handle,
    dir: PathBuf,
    db: Option<Arc<Mutex<Connection>>>,
    // What's on the clipboard now, so deleting it also clears it.
    current: Option<String>,
}

impl Service {
    pub fn new(bus: Bus, clip: datacontrol::Handle) -> Self {
        Self {
            bus,
            clip,
            dir: config::data_dir().join("clipboard-data"),
            db: None,
            current: None,
        }
    }

    pub async fn run(
        mut self,
        mut rx: mpsc::Receiver<Request>,
        mut selections: mpsc::Receiver<Selection>,
    ) {
        let path = config::data_dir().join("clipboard.db");
        let dir = self.dir.clone();
        let opened = tokio::task::spawn_blocking(move || open(&path, &dir)).await;
        match opened {
            Ok(Ok(conn)) => self.db = Some(Arc::new(Mutex::new(conn))),
            // Left as it is on disk: a history that won't open is one to look
            // at, never one to start over.
            Ok(Err(e)) => return tracing::error!("clipboard database: {e:#}"),
            Err(e) => return tracing::error!("clipboard database: {e}"),
        }
        self.publish().await;

        loop {
            tokio::select! {
                selection = selections.recv() => match selection {
                    Some(s) => self.capture(s).await,
                    None => return,
                },
                req = rx.recv() => match req {
                    Some(req) => {
                        let cmd = req.cmd.clone();
                        match Command::try_from(req) {
                            Ok(c) => {
                                if let Err(e) = self.handle(c).await {
                                    tracing::warn!(cmd, "clipboard: {e:#}");
                                }
                            }
                            Err(e) => tracing::warn!(cmd, "bad command: {e}"),
                        }
                    }
                    None => return,
                },
            }
        }
    }

    // Database work leaves the runtime: a commit waits on the disk, and every
    // service shares this thread.
    async fn db<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let db = self.db.clone().context("database closed")?;
        tokio::task::spawn_blocking(move || f(&mut db.lock().unwrap())).await?
    }

    async fn publish(&self) {
        match self.db(|c| list(c)).await {
            Ok(items) => self.bus.publish(SERVICE, &json!({ "items": items })),
            Err(e) => tracing::warn!("clipboard list: {e:#}"),
        }
    }

    async fn capture(&mut self, selection: Selection) {
        let dir = self.dir.clone();
        match self.db(move |c| store(c, &dir, &selection)).await {
            Ok(Some(hash)) => {
                self.current = Some(hash);
                self.publish().await;
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("clipboard store: {e:#}"),
        }
    }

    async fn handle(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Copy { id } => {
                let found = self.db(move |c| source(c, &id)).await?;
                match found {
                    Some(Source::Text(text)) => self.clip.text(&text),
                    Some(Source::Uri(text)) => self.clip.bytes(URI, text.as_bytes()),
                    Some(Source::File(path, mime)) => {
                        self.clip.bytes(&mime, &tokio::fs::read(path).await?);
                    }
                    None => {}
                }
                return Ok(());
            }
            Command::Text { text } => {
                self.clip.text(&text);
                return Ok(());
            }
            Command::File { path, mime } => {
                self.clip.bytes(&mime, &tokio::fs::read(path).await?);
                return Ok(());
            }
            Command::Content { id } => {
                let key = id.clone();
                let text = self.db(move |c| content(c, &key)).await?;
                self.bus
                    .event(SERVICE, "content", &json!({ "id": id, "text": text }));
                return Ok(());
            }
            Command::Search { query } => {
                let q = query.clone();
                let items = self.db(move |c| search(c, &q)).await?;
                self.bus.event(
                    SERVICE,
                    "results",
                    &json!({ "query": query, "items": items }),
                );
                return Ok(());
            }
            Command::Delete { id } => {
                let dir = self.dir.clone();
                let hash = self.db(move |c| delete(c, &dir, &id)).await?;
                if hash.is_some() && hash == self.current {
                    self.current = None;
                    self.clip.clear();
                }
            }
            Command::Clear => {
                let dir = self.dir.clone();
                self.db(move |c| clear(c, &dir)).await?;
                self.current = None;
                self.clip.clear();
            }
            Command::Pin { id } => self.db(move |c| toggle_pin(c, &id)).await?,
            Command::Alias { id, alias } => self.db(move |c| set_alias(c, &id, &alias)).await?,
            Command::Swap { a, b } => self.db(move |c| swap(c, &a, &b)).await?,
        }
        self.publish().await;
        Ok(())
    }
}

fn open(path: &Path, dir: &Path) -> Result<Connection> {
    std::fs::create_dir_all(dir)?;
    let mut c = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    c.busy_timeout(BUSY_TIMEOUT)?;
    c.execute_batch(SCHEMA)?;
    migrate(&mut c, dir)?;
    Ok(c)
}

// Safe to run on every start: after the first, neither step finds anything.
fn migrate(c: &mut Connection, dir: &Path) -> Result<()> {
    let indexed: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'clipboard_fts')",
        [],
        |r| r.get(0),
    )?;

    let t = c.transaction()?;
    // Nothing reads the old full-text index, and each copy paid to rewrite it.
    t.execute_batch(
        "DROP TRIGGER IF EXISTS clipboard_items_ai;
         DROP TRIGGER IF EXISTS clipboard_items_ad;
         DROP TRIGGER IF EXISTS clipboard_items_au;
         DROP TABLE IF EXISTS clipboard_fts;",
    )?;

    // Rows from before the rename point at a folder that no longer holds them.
    let stale: Vec<(i64, String)> = {
        let mut stmt =
            t.prepare("SELECT id, binary_path FROM clipboard_items WHERE is_image = 1")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut moved = 0;
    for (id, old) in stale {
        let path = Path::new(&old);
        let Some(name) = path.file_name() else {
            continue;
        };
        let new = dir.join(name);
        if path != new && new.is_file() {
            let new = new.to_string_lossy();
            t.execute(
                "UPDATE clipboard_items SET binary_path = ?1 WHERE id = ?2",
                params![new, id],
            )?;
            moved += 1;
        }
    }
    t.commit()?;

    if moved > 0 {
        tracing::info!(moved, "clipboard image paths moved");
    }
    if indexed {
        c.execute_batch("VACUUM")?;
    }
    Ok(())
}

// A row's text may be stored as a blob: the old scripts wrote through readfile().
fn text(row: &Row, i: usize) -> rusqlite::Result<String> {
    Ok(match row.get_ref(i)? {
        ValueRef::Text(b) | ValueRef::Blob(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    })
}

fn item(row: &Row) -> rusqlite::Result<Item> {
    let mime = text(row, 1)?;
    Ok(Item {
        id: row.get::<_, i64>(0)?.to_string(),
        is_file: mime == URI,
        mime,
        preview: text(row, 2)?,
        is_image: row.get::<_, i64>(3)? == 1,
        binary_path: text(row, 4)?,
        hash: text(row, 5)?,
        size: row.get(6)?,
        created_at: row.get(7)?,
        pinned: row.get::<_, i64>(8)? == 1,
        alias: text(row, 9)?,
        display_index: row.get::<_, Option<i64>>(10)?.unwrap_or(-1),
    })
}

fn list(c: &Connection) -> Result<Vec<Item>> {
    let sql = format!("SELECT {COLUMNS} FROM clipboard_items {ORDER} LIMIT ?1");
    let mut stmt = c.prepare_cached(&sql)?;
    let items = stmt.query_map([LIST_SIZE], item)?;
    Ok(items.collect::<rusqlite::Result<_>>()?)
}

fn search(c: &Connection, query: &str) -> Result<Vec<Item>> {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let sql = format!(
        "SELECT {COLUMNS} FROM clipboard_items
         WHERE CAST(preview AS TEXT) LIKE ?1 ESCAPE '\\'
            OR CAST(full_content AS TEXT) LIKE ?1 ESCAPE '\\'
            OR alias LIKE ?1 ESCAPE '\\'
         {ORDER} LIMIT ?2"
    );
    let mut stmt = c.prepare_cached(&sql)?;
    let items = stmt.query_map(params![format!("%{escaped}%"), SEARCH_SIZE], item)?;
    Ok(items.collect::<rusqlite::Result<_>>()?)
}

fn clean(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace(['\r', '\0'], "");
    text.trim_end_matches('\n').to_string()
}

fn preview(content: &str) -> String {
    if content.chars().count() > PREVIEW_CHARS {
        let mut cut: String = content.chars().take(PREVIEW_CHARS - 3).collect();
        cut.push_str("...");
        cut
    } else {
        content.to_string()
    }
}

fn file_size(uri: &str) -> i64 {
    uri.strip_prefix("file://")
        .and_then(|p| std::fs::metadata(&*percent_decode_str(p).decode_utf8_lossy()).ok())
        .map_or(0, |m| m.len() as i64)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

fn parse(selection: &Selection) -> Option<New<'_>> {
    let mime = selection.mime.as_str();
    if mime.starts_with("image/") {
        if selection.bytes.is_empty() {
            return None;
        }
        return Some(New {
            hash: hash(&selection.bytes),
            mime,
            preview: "[Image]".to_string(),
            content: String::new(),
            size: selection.bytes.len() as i64,
            blob: Some(&selection.bytes),
        });
    }

    let content = clean(&selection.bytes);
    if content.is_empty() {
        return None;
    }
    let is_uri = mime == URI;
    Some(New {
        hash: hash(content.as_bytes()),
        mime: if is_uri { URI } else { "text/plain" },
        preview: preview(&content),
        size: if is_uri {
            file_size(&content)
        } else {
            content.chars().count() as i64
        },
        content,
        blob: None,
    })
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        _ => "img",
    }
}

// The file lands before its row, so a row never points at nothing. Named by
// content, so the same image copied twice is one file.
fn keep_blob(c: &Connection, dir: &Path, new: &New, data: &[u8]) -> Result<String> {
    let known: Option<String> = c
        .query_row(
            "SELECT binary_path FROM clipboard_items WHERE content_hash = ?1",
            [&new.hash],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(path) = known.filter(|p| Path::new(p).is_file()) {
        return Ok(path);
    }

    let path = dir.join(format!("clipboard_{}.{}", new.hash, extension(new.mime)));
    let tmp = path.with_extension("part");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    std::fs::rename(tmp, &path)?;
    Ok(path.to_string_lossy().into_owned())
}

fn reindex(t: &Transaction) -> Result<()> {
    t.execute_batch(
        "UPDATE clipboard_items SET display_index = r.idx
         FROM (SELECT id, ROW_NUMBER() OVER (
                   PARTITION BY pinned ORDER BY display_index ASC, updated_at DESC, id DESC
               ) - 1 AS idx FROM clipboard_items) r
         WHERE clipboard_items.id = r.id AND clipboard_items.display_index IS NOT r.idx;",
    )?;
    Ok(())
}

fn store(c: &mut Connection, dir: &Path, selection: &Selection) -> Result<Option<String>> {
    let Some(new) = parse(selection) else {
        return Ok(None);
    };
    let binary = match new.blob {
        Some(data) => keep_blob(c, dir, &new, data)?,
        None => String::new(),
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let t = c.transaction()?;
    t.execute(
        "UPDATE clipboard_items SET display_index = display_index + 1 WHERE pinned = 0",
        [],
    )?;
    t.execute(
        "INSERT INTO clipboard_items (content_hash, mime_type, preview, full_content, is_image,
             binary_path, size, pinned, display_index, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8, ?8)
         ON CONFLICT(content_hash) DO UPDATE SET
             updated_at = excluded.updated_at,
             display_index = 0,
             binary_path = excluded.binary_path",
        params![
            new.hash,
            new.mime,
            new.preview,
            new.content,
            new.blob.is_some(),
            binary,
            new.size,
            now
        ],
    )?;
    reindex(&t)?;
    t.commit()?;
    Ok(Some(new.hash))
}

enum Source {
    Text(String),
    Uri(String),
    File(String, String),
}

fn source(c: &Connection, id: &str) -> Result<Option<Source>> {
    Ok(c.query_row(
        "SELECT mime_type, is_image, binary_path, full_content FROM clipboard_items WHERE id = ?1",
        [id],
        |r| {
            let mime = text(r, 0)?;
            Ok(if r.get::<_, i64>(1)? == 1 {
                Source::File(text(r, 2)?, mime)
            } else if mime == URI {
                Source::Uri(text(r, 3)?.replace('\r', ""))
            } else {
                Source::Text(text(r, 3)?)
            })
        },
    )
    .optional()?)
}

fn content(c: &Connection, id: &str) -> Result<String> {
    let mut text = c
        .query_row(
            "SELECT full_content FROM clipboard_items WHERE id = ?1",
            [id],
            |r| text(r, 0),
        )
        .optional()?
        .unwrap_or_default();
    if text.len() > CONTENT_MAX {
        let mut end = CONTENT_MAX;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Ok(text)
}

fn delete(c: &mut Connection, dir: &Path, id: &str) -> Result<Option<String>> {
    let row = c
        .query_row(
            "SELECT content_hash, is_image FROM clipboard_items WHERE id = ?1",
            [id],
            |r| Ok((text(r, 0)?, r.get::<_, i64>(1)? == 1)),
        )
        .optional()?;
    let Some((hash, image)) = row else {
        return Ok(None);
    };
    c.execute("DELETE FROM clipboard_items WHERE id = ?1", [id])?;
    if image {
        sweep(c, dir)?;
    }
    Ok(Some(hash))
}

fn clear(c: &mut Connection, dir: &Path) -> Result<()> {
    c.execute("DELETE FROM clipboard_items WHERE pinned = 0", [])?;
    let t = c.transaction()?;
    reindex(&t)?;
    t.commit()?;
    sweep(c, dir)
}

// Compared by file name, so a row whose path still differs protects its file.
fn sweep(c: &Connection, dir: &Path) -> Result<()> {
    let mut stmt = c.prepare("SELECT binary_path FROM clipboard_items WHERE is_image = 1")?;
    let kept: HashSet<String> = stmt
        .query_map([], |r| text(r, 0))?
        .filter_map(|p| {
            let path = p.ok()?;
            Some(Path::new(&path).file_name()?.to_string_lossy().into_owned())
        })
        .collect();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("clipboard_") && !kept.contains(&name) {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn toggle_pin(c: &mut Connection, id: &str) -> Result<()> {
    let t = c.transaction()?;
    t.execute(
        "UPDATE clipboard_items SET pinned = 1 - pinned WHERE id = ?1",
        [id],
    )?;
    t.execute(
        "UPDATE clipboard_items SET display_index = CASE WHEN id = ?1 THEN 0 ELSE display_index + 1 END
         WHERE pinned = (SELECT pinned FROM clipboard_items WHERE id = ?1)",
        [id],
    )?;
    reindex(&t)?;
    t.commit()?;
    Ok(())
}

fn set_alias(c: &Connection, id: &str, alias: &str) -> Result<()> {
    let alias = Some(alias.trim()).filter(|a| !a.is_empty());
    c.execute(
        "UPDATE clipboard_items SET alias = ?1 WHERE id = ?2",
        params![alias, id],
    )?;
    Ok(())
}

fn swap(c: &mut Connection, a: &str, b: &str) -> Result<()> {
    let t = c.transaction()?;
    reindex(&t)?;
    let index = |id: &str| -> rusqlite::Result<i64> {
        t.query_row(
            "SELECT display_index FROM clipboard_items WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
    };
    let (ia, ib) = (index(a)?, index(b)?);
    t.execute(
        "UPDATE clipboard_items SET display_index = ?1 WHERE id = ?2",
        params![ib, a],
    )?;
    t.execute(
        "UPDATE clipboard_items SET display_index = ?1 WHERE id = ?2",
        params![ia, b],
    )?;
    t.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        root: PathBuf,
        dir: PathBuf,
        c: Connection,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("flokd-clip-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("clipboard-data");
        let c = open(&root.join("clipboard.db"), &dir).unwrap();
        Scratch { root, dir, c }
    }

    fn copy(s: &mut Scratch, mime: &str, bytes: &[u8]) -> Option<String> {
        let selection = Selection {
            mime: mime.to_string(),
            bytes: bytes.to_vec(),
        };
        store(&mut s.c, &s.dir, &selection).unwrap()
    }

    fn previews(s: &Scratch) -> Vec<String> {
        list(&s.c).unwrap().into_iter().map(|i| i.preview).collect()
    }

    fn id_of(s: &Scratch, preview: &str) -> String {
        list(&s.c)
            .unwrap()
            .into_iter()
            .find(|i| i.preview == preview)
            .unwrap()
            .id
    }

    #[test]
    fn hashes_what_the_scripts_hashed() {
        let selection = Selection {
            mime: "text/plain".to_string(),
            bytes: b"hello\n\n".to_vec(),
        };
        let new = parse(&selection).unwrap();
        // printf hello | md5sum
        assert_eq!(new.hash, "5d41402abc4b2a76b9719d911017c592");
        assert_eq!(new.content, "hello");
        assert_eq!(new.size, 5);
    }

    #[test]
    fn nothing_to_keep_from_empty_text() {
        let mut s = scratch("empty");
        assert_eq!(copy(&mut s, "text/plain", b"\n\n"), None);
        assert!(list(&s.c).unwrap().is_empty());
    }

    #[test]
    fn carriage_returns_go_before_hashing_so_a_copy_back_matches() {
        let mut s = scratch("cr");
        copy(&mut s, "text/plain", b"a\r\nb\r\n");
        copy(&mut s, "text/plain", b"a\nb");
        assert_eq!(previews(&s), ["a\nb"]);
    }

    #[test]
    fn long_text_gets_a_short_preview_and_keeps_all_of_it() {
        let mut s = scratch("long");
        let long = "é".repeat(150);
        copy(&mut s, "text/plain", long.as_bytes());
        let item = &list(&s.c).unwrap()[0];
        assert_eq!(item.preview.chars().count(), 100);
        assert!(item.preview.ends_with("..."));
        assert_eq!(item.size, 150);
        assert_eq!(content(&s.c, &item.id).unwrap(), long);
    }

    #[test]
    fn content_is_cut_on_a_character() {
        let mut s = scratch("cap");
        let long = "é".repeat(CONTENT_MAX);
        copy(&mut s, "text/plain", long.as_bytes());
        let id = list(&s.c).unwrap()[0].id.clone();
        let text = content(&s.c, &id).unwrap();
        assert!(text.len() <= CONTENT_MAX && text.len() > CONTENT_MAX - 2);
        match source(&s.c, &id).unwrap().unwrap() {
            Source::Text(full) => assert_eq!(full, long),
            _ => panic!("not text"),
        }
    }

    #[test]
    fn a_new_copy_goes_first_and_an_old_one_comes_back_up() {
        let mut s = scratch("order");
        copy(&mut s, "text/plain", b"one");
        copy(&mut s, "text/plain", b"two");
        copy(&mut s, "text/plain", b"three");
        assert_eq!(previews(&s), ["three", "two", "one"]);
        copy(&mut s, "text/plain", b"one");
        assert_eq!(previews(&s), ["one", "three", "two"]);
        let count: i64 =
            s.c.query_row("SELECT COUNT(*) FROM clipboard_items", [], |r| r.get(0))
                .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn the_same_image_twice_is_one_file() {
        let mut s = scratch("image");
        copy(&mut s, "image/png", b"pixels");
        copy(&mut s, "image/png", b"pixels");
        assert_eq!(std::fs::read_dir(&s.dir).unwrap().count(), 1);
        let item = &list(&s.c).unwrap()[0];
        assert!(item.is_image);
        assert_eq!(item.preview, "[Image]");
        assert_eq!(std::fs::read(&item.binary_path).unwrap(), b"pixels");
    }

    #[test]
    fn an_image_whose_file_went_missing_gets_it_back() {
        let mut s = scratch("regrow");
        copy(&mut s, "image/png", b"pixels");
        let path = list(&s.c).unwrap()[0].binary_path.clone();
        std::fs::remove_file(&path).unwrap();
        copy(&mut s, "image/png", b"pixels");
        assert_eq!(std::fs::read(&path).unwrap(), b"pixels");
    }

    #[test]
    fn a_file_is_marked_and_sized() {
        let mut s = scratch("uri");
        let file = s.root.join("a file.txt");
        std::fs::write(&file, "12345").unwrap();
        let uri = format!("file://{}", file.display()).replace(' ', "%20");
        copy(&mut s, URI, format!("{uri}\r\n").as_bytes());
        let item = &list(&s.c).unwrap()[0];
        assert!(item.is_file);
        assert_eq!(item.size, 5);
        assert_eq!(item.preview, uri);
    }

    #[test]
    fn search_reaches_past_the_list_and_into_the_middle_of_words() {
        let mut s = scratch("search");
        let long = format!("{}needle in the tail", "x".repeat(300));
        copy(&mut s, "text/plain", long.as_bytes());
        for i in 0..150 {
            copy(&mut s, "text/plain", format!("filler {i}").as_bytes());
        }
        assert!(
            list(&s.c)
                .unwrap()
                .iter()
                .all(|i| !i.preview.starts_with('x'))
        );
        assert_eq!(search(&s.c, "eedl").unwrap().len(), 1);
        assert_eq!(search(&s.c, "NEEDLE").unwrap().len(), 1);
    }

    #[test]
    fn search_takes_percent_and_underscore_literally() {
        let mut s = scratch("like");
        copy(&mut s, "text/plain", b"100% sure");
        copy(&mut s, "text/plain", b"snake_case");
        copy(&mut s, "text/plain", b"snakeXcase");
        assert_eq!(search(&s.c, "%").unwrap().len(), 1);
        assert_eq!(search(&s.c, "e_c").unwrap().len(), 1);
    }

    #[test]
    fn search_finds_an_alias() {
        let mut s = scratch("alias");
        copy(&mut s, "text/plain", b"hunter2");
        let id = id_of(&s, "hunter2");
        set_alias(&s.c, &id, "  work thing ").unwrap();
        assert_eq!(search(&s.c, "work").unwrap()[0].alias, "work thing");
        set_alias(&s.c, &id, "   ").unwrap();
        assert_eq!(list(&s.c).unwrap()[0].alias, "");
    }

    #[test]
    fn an_alias_is_only_ever_data() {
        let mut s = scratch("inject");
        copy(&mut s, "text/plain", b"x");
        let id = id_of(&s, "x");
        let alias = "$(touch /tmp/nope)'; DROP TABLE clipboard_items; --";
        set_alias(&s.c, &id, alias).unwrap();
        assert_eq!(list(&s.c).unwrap()[0].alias, alias);
    }

    #[test]
    fn pinning_leads_the_list_and_survives_clear() {
        let mut s = scratch("pin");
        copy(&mut s, "text/plain", b"one");
        copy(&mut s, "text/plain", b"two");
        let one = id_of(&s, "one");
        toggle_pin(&mut s.c, &one).unwrap();
        assert_eq!(previews(&s), ["one", "two"]);
        clear(&mut s.c, &s.dir).unwrap();
        assert_eq!(previews(&s), ["one"]);
        toggle_pin(&mut s.c, &one).unwrap();
        assert!(!list(&s.c).unwrap()[0].pinned);
    }

    #[test]
    fn clear_keeps_the_image_of_a_pinned_row() {
        let mut s = scratch("clear");
        copy(&mut s, "image/png", b"keep");
        let pinned = list(&s.c).unwrap()[0].clone();
        toggle_pin(&mut s.c, &pinned.id).unwrap();
        copy(&mut s, "image/png", b"drop");
        let stray = s.dir.join("clipboard_stray.png");
        std::fs::write(&stray, "x").unwrap();
        clear(&mut s.c, &s.dir).unwrap();
        assert!(Path::new(&pinned.binary_path).is_file());
        assert!(!stray.exists());
        assert_eq!(std::fs::read_dir(&s.dir).unwrap().count(), 1);
    }

    #[test]
    fn deleting_an_image_takes_its_file() {
        let mut s = scratch("delete");
        copy(&mut s, "image/png", b"pixels");
        let item = list(&s.c).unwrap()[0].clone();
        let hash = delete(&mut s.c, &s.dir, &item.id).unwrap();
        assert_eq!(hash.as_deref(), Some(item.hash.as_str()));
        assert!(!Path::new(&item.binary_path).exists());
        assert_eq!(delete(&mut s.c, &s.dir, &item.id).unwrap(), None);
    }

    #[test]
    fn swap_trades_two_places() {
        let mut s = scratch("swap");
        copy(&mut s, "text/plain", b"one");
        copy(&mut s, "text/plain", b"two");
        copy(&mut s, "text/plain", b"three");
        let (a, b) = (id_of(&s, "three"), id_of(&s, "two"));
        swap(&mut s.c, &a, &b).unwrap();
        assert_eq!(previews(&s), ["two", "three", "one"]);
    }

    // The database as the old scripts left it: readfile() blobs, the FTS
    // index and its triggers, and a path from before the rename.
    #[test]
    fn migrates_an_old_database_and_changes_nothing_else() {
        let root = std::env::temp_dir().join(format!("flokd-clip-{}-mig", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("clipboard-data");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("clipboard_1.png"), "x").unwrap();
        let path = root.join("clipboard.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            c.execute_batch(
                "CREATE VIRTUAL TABLE clipboard_fts USING fts5(preview, full_content,
                     content=clipboard_items, content_rowid=id);
                 CREATE TRIGGER clipboard_items_ai AFTER INSERT ON clipboard_items BEGIN
                     INSERT INTO clipboard_fts(rowid, preview, full_content)
                     VALUES (new.id, new.preview, new.full_content);
                 END;
                 INSERT INTO clipboard_items (content_hash, mime_type, preview, full_content,
                     is_image, binary_path, size, pinned, display_index, created_at, updated_at)
                 VALUES
                 ('a', 'image/png', x'5B496D6167655D', x'', 1,
                  '/old/ambxst/clipboard-data/clipboard_1.png', 1, 1, 0, 5, 6),
                 ('b', 'image/png', x'5B496D6167655D', x'', 1,
                  '/old/ambxst/clipboard-data/clipboard_gone.png', 1, 0, 0, 5, 6),
                 ('c', 'text/plain', x'6869', x'6869', 0, '', 2, 0, 1, 7, 8);",
            )
            .unwrap();
        }

        let c = open(&path, &dir).unwrap();
        let again = open(&path, &dir).unwrap();
        drop(again);

        let names: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'clipboard_fts%'
                     OR type = 'trigger'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(names, 0);

        let items = list(&c).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0].binary_path,
            dir.join("clipboard_1.png").to_string_lossy()
        );
        assert!(items[0].pinned);
        // Its file is gone, so its path stays what it was.
        assert!(items[1].binary_path.starts_with("/old/ambxst"));
        assert_eq!(items[2].preview, "hi");
        assert_eq!(content(&c, &items[2].id).unwrap(), "hi");
        assert_eq!(search(&c, "hi").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    // For a copy of the real history: CLIP_DB=... CLIP_DIR=... cargo test -- --ignored
    #[test]
    #[ignore]
    fn migrate_copy() {
        let db = std::env::var("CLIP_DB").unwrap();
        let dir = std::env::var("CLIP_DIR").unwrap();
        open(Path::new(&db), Path::new(&dir)).unwrap();
    }
}
