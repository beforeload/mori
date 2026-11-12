use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, OptionalExtension, params};

use super::model::*;
use super::parse;
use super::time;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memos(
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,
    pinned INTEGER NOT NULL DEFAULT 0,
    source TEXT NOT NULL DEFAULT 'app',
    images TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS memo_versions(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memo_id TEXT NOT NULL,
    content TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS memo_versions_memo ON memo_versions(memo_id);
CREATE TABLE IF NOT EXISTS tag_meta(
    tag TEXT PRIMARY KEY,
    pinned INTEGER NOT NULL DEFAULT 0,
    icon TEXT
);
CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS embeddings(
    memo_id TEXT NOT NULL,
    model TEXT NOT NULL,
    hash TEXT NOT NULL,
    vector BLOB NOT NULL,
    PRIMARY KEY(memo_id, model)
);
CREATE TABLE IF NOT EXISTS insights(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    perspective TEXT NOT NULL,
    scope TEXT NOT NULL,
    memo_ids TEXT NOT NULL,
    content TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_messages(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    refs TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL
);
"#;

const VERSION_RETENTION_DAYS: i64 = 30;

pub struct NewMemo {
    pub content: String,
    pub created_at: Option<i64>,
    pub images: Vec<String>,
    pub source: String,
}

pub struct Store {
    conn: Connection,
    data_dir: PathBuf,
    memos: Vec<Memo>,
    index: HashMap<String, usize>,
    tag_meta: BTreeMap<String, TagMeta>,
    settings: Settings,
    data_version: i64,
    revision: u64,
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MORI_DATA_DIR").or_else(|_| std::env::var("MINDFLOW_DATA_DIR")) {
        return PathBuf::from(dir);
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    let mori = base.join("Mori");
    let old = base.join("MindFlow");
    if !mori.exists() && old.exists() && std::fs::rename(&old, &mori).is_err() {
        return old;
    }
    mori
}

/// `mori.db`, or the legacy `mindflow.db` if that is the file still on disk.
pub fn database_path(dir: &Path) -> PathBuf {
    let mori = dir.join("mori.db");
    let old = dir.join("mindflow.db");
    if !mori.exists() && old.exists() {
        let _ = std::fs::rename(&old, &mori);
        let _ = std::fs::rename(dir.join("mindflow.db-wal"), dir.join("mori.db-wal"));
        let _ = std::fs::rename(dir.join("mindflow.db-shm"), dir.join("mori.db-shm"));
    }
    if mori.exists() || !old.exists() { mori } else { old }
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

pub fn content_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

impl Store {
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir.join("images"))
            .with_context(|| t!("无法创建数据目录 {}", data_dir.display()))?;
        let conn = Connection::open(database_path(data_dir))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA)?;
        let mut store = Self {
            conn,
            data_dir: data_dir.to_path_buf(),
            memos: Vec::new(),
            index: HashMap::new(),
            tag_meta: BTreeMap::new(),
            settings: Settings::default(),
            data_version: 0,
            revision: 0,
        };
        store.prune_versions()?;
        store.reload()?;
        Ok(store)
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&default_data_dir())
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn images_dir(&self) -> PathBuf {
        self.data_dir.join("images")
    }

    pub fn image_path(&self, name: &str) -> PathBuf {
        self.images_dir().join(name)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn bump(&mut self) {
        self.revision += 1;
        self.data_version = self.read_data_version();
    }

    fn read_data_version(&self) -> i64 {
        self.conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Reloads everything when another process (MCP server, local API) wrote to the database.
    pub fn sync_external_changes(&mut self) -> Result<bool> {
        let version = self.read_data_version();
        if version == self.data_version {
            return Ok(false);
        }
        self.reload()?;
        Ok(true)
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, created_at, updated_at, deleted_at, pinned, source, images FROM memos",
        )?;
        let rows = stmt.query_map([], |r| {
            let images: String = r.get(7)?;
            Ok(Memo {
                id: r.get(0)?,
                content: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                deleted_at: r.get(4)?,
                pinned: r.get::<_, i64>(5)? != 0,
                source: r.get(6)?,
                images: serde_json::from_str(&images).unwrap_or_default(),
                tags: Vec::new(),
                refs: Vec::new(),
                char_count: 0,
                has_link: false,
            })
        })?;
        let mut memos = Vec::new();
        for row in rows {
            let mut memo = row?;
            memo.refresh_derived();
            memos.push(memo);
        }
        drop(stmt);
        memos.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        self.index = memos
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.clone(), i))
            .collect();
        self.memos = memos;

        let mut stmt = self.conn.prepare("SELECT tag, pinned, icon FROM tag_meta")?;
        self.tag_meta = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    TagMeta {
                        pinned: r.get::<_, i64>(1)? != 0,
                        icon: r.get(2)?,
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        drop(stmt);

        let saved = self.kv_get("settings");
        self.settings = match saved.as_deref() {
            Some(json) => serde_json::from_str(json).unwrap_or_default(),
            None => {
                let mut settings = Settings::default();
                settings.language = crate::i18n::system_language();
                if settings.language == crate::i18n::Language::En {
                    settings.nickname = "Me".into();
                }
                settings
            }
        };
        crate::i18n::set_language(self.settings.language);
        self.bump();
        Ok(())
    }

    fn reindex(&mut self) {
        self.memos.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        self.index = self
            .memos
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.clone(), i))
            .collect();
    }

    // ---------------------------------------------------------------- memos

    pub fn active_memos(&self) -> impl Iterator<Item = &Memo> + Clone {
        self.memos.iter().filter(|m| !m.is_deleted())
    }

    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        self.active_memos().count()
    }

    pub fn trashed_memos(&self) -> Vec<&Memo> {
        let mut list: Vec<&Memo> = self.memos.iter().filter(|m| m.is_deleted()).collect();
        list.sort_by_key(|m| std::cmp::Reverse(m.deleted_at));
        list
    }

    pub fn get(&self, id: &str) -> Option<&Memo> {
        self.index.get(id).map(|&i| &self.memos[i])
    }

    pub fn get_active(&self, id: &str) -> Option<&Memo> {
        self.get(id).filter(|m| !m.is_deleted())
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Memo> {
        self.index.get(id).copied().map(move |i| &mut self.memos[i])
    }

    pub fn backlinks(&self, id: &str) -> Vec<&Memo> {
        self.active_memos()
            .filter(|m| m.refs.iter().any(|r| r == id))
            .collect()
    }

    pub fn create_memo(&mut self, new: NewMemo) -> Result<String> {
        let ids = self.create_many(vec![new])?;
        Ok(ids.into_iter().next().unwrap())
    }

    pub fn create_many(&mut self, items: Vec<NewMemo>) -> Result<Vec<String>> {
        let now = time::now();
        let tx = self.conn.transaction()?;
        let mut created = Vec::with_capacity(items.len());
        {
            let mut stmt = tx.prepare(
                "INSERT INTO memos(id, content, created_at, updated_at, pinned, source, images) VALUES(?1, ?2, ?3, ?4, 0, ?5, ?6)",
            )?;
            for item in items {
                let id = new_id();
                let created_at = item.created_at.unwrap_or(now);
                let images = serde_json::to_string(&item.images)?;
                stmt.execute(params![
                    id,
                    item.content,
                    created_at,
                    created_at,
                    item.source,
                    images
                ])?;
                let mut memo = Memo {
                    id: id.clone(),
                    content: item.content,
                    created_at,
                    updated_at: created_at,
                    deleted_at: None,
                    pinned: false,
                    source: item.source,
                    images: item.images,
                    tags: Vec::new(),
                    refs: Vec::new(),
                    char_count: 0,
                    has_link: false,
                };
                memo.refresh_derived();
                created.push(memo);
            }
        }
        tx.commit()?;
        let ids = created.iter().map(|m| m.id.clone()).collect();
        self.memos.extend(created);
        self.reindex();
        self.bump();
        Ok(ids)
    }

    pub fn update_memo(&mut self, id: &str, content: String, images: Vec<String>) -> Result<()> {
        let Some(memo) = self.get(id) else {
            anyhow::bail!(t!("笔记不存在"));
        };
        if memo.content == content && memo.images == images {
            return Ok(());
        }
        let now = time::now();
        let previous = memo.content.clone();
        if previous != content {
            self.conn.execute(
                "INSERT INTO memo_versions(memo_id, content, saved_at) VALUES(?1, ?2, ?3)",
                params![id, previous, now],
            )?;
        }
        self.conn.execute(
            "UPDATE memos SET content = ?2, images = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, content, serde_json::to_string(&images)?, now],
        )?;
        let memo = self.get_mut(id).unwrap();
        memo.content = content;
        memo.images = images;
        memo.updated_at = now;
        memo.refresh_derived();
        self.bump();
        Ok(())
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE memos SET pinned = ?2 WHERE id = ?1",
            params![id, pinned as i64],
        )?;
        if let Some(m) = self.get_mut(id) {
            m.pinned = pinned;
        }
        self.bump();
        Ok(())
    }

    pub fn set_created_at(&mut self, id: &str, created_at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE memos SET created_at = ?2 WHERE id = ?1",
            params![id, created_at],
        )?;
        if let Some(m) = self.get_mut(id) {
            m.created_at = created_at;
        }
        self.reindex();
        self.bump();
        Ok(())
    }

    pub fn trash(&mut self, id: &str) -> Result<()> {
        let now = time::now();
        self.conn.execute(
            "UPDATE memos SET deleted_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        if let Some(m) = self.get_mut(id) {
            m.deleted_at = Some(now);
        }
        self.bump();
        Ok(())
    }

    pub fn restore(&mut self, id: &str) -> Result<()> {
        self.conn
            .execute("UPDATE memos SET deleted_at = NULL WHERE id = ?1", params![id])?;
        if let Some(m) = self.get_mut(id) {
            m.deleted_at = None;
        }
        self.bump();
        Ok(())
    }

    pub fn purge(&mut self, id: &str) -> Result<()> {
        let images = self.get(id).map(|m| m.images.clone()).unwrap_or_default();
        self.conn.execute("DELETE FROM memos WHERE id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM memo_versions WHERE memo_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM embeddings WHERE memo_id = ?1", params![id])?;
        for image in images {
            let _ = std::fs::remove_file(self.image_path(&image));
        }
        self.memos.retain(|m| m.id != id);
        self.reindex();
        self.bump();
        Ok(())
    }

    pub fn empty_trash(&mut self) -> Result<usize> {
        let ids: Vec<String> = self.trashed_memos().iter().map(|m| m.id.clone()).collect();
        for id in &ids {
            self.purge(id)?;
        }
        Ok(ids.len())
    }

    // ------------------------------------------------------------- versions

    fn prune_versions(&self) -> Result<()> {
        let cutoff = time::now() - VERSION_RETENTION_DAYS * 86_400;
        self.conn
            .execute("DELETE FROM memo_versions WHERE saved_at < ?1", params![cutoff])?;
        Ok(())
    }

    pub fn versions(&self, memo_id: &str) -> Result<Vec<MemoVersion>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, saved_at FROM memo_versions WHERE memo_id = ?1 ORDER BY saved_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![memo_id], |r| {
            Ok(MemoVersion {
                id: r.get(0)?,
                content: r.get(1)?,
                saved_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn restore_version(&mut self, memo_id: &str, version_id: i64) -> Result<()> {
        let content: String = self.conn.query_row(
            "SELECT content FROM memo_versions WHERE id = ?1 AND memo_id = ?2",
            params![version_id, memo_id],
            |r| r.get(0),
        )?;
        let images = self.get(memo_id).map(|m| m.images.clone()).unwrap_or_default();
        self.update_memo(memo_id, content, images)
    }

    // ----------------------------------------------------------------- tags

    pub fn tag_meta(&self, tag: &str) -> TagMeta {
        self.tag_meta.get(tag).cloned().unwrap_or_default()
    }

    pub fn pinned_tags(&self) -> Vec<String> {
        self.tag_meta
            .iter()
            .filter(|(_, m)| m.pinned)
            .map(|(t, _)| t.clone())
            .collect()
    }

    fn write_tag_meta(&mut self, tag: &str, meta: TagMeta) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tag_meta(tag, pinned, icon) VALUES(?1, ?2, ?3)
             ON CONFLICT(tag) DO UPDATE SET pinned = excluded.pinned, icon = excluded.icon",
            params![tag, meta.pinned as i64, meta.icon],
        )?;
        self.tag_meta.insert(tag.to_string(), meta);
        self.bump();
        Ok(())
    }

    pub fn set_tag_pinned(&mut self, tag: &str, pinned: bool) -> Result<()> {
        let mut meta = self.tag_meta(tag);
        meta.pinned = pinned;
        self.write_tag_meta(tag, meta)
    }

    pub fn set_tag_icon(&mut self, tag: &str, icon: Option<String>) -> Result<()> {
        let mut meta = self.tag_meta(tag);
        meta.icon = icon.filter(|s| !s.trim().is_empty());
        self.write_tag_meta(tag, meta)
    }

    fn memos_with_tag(&self, tag: &str) -> Vec<String> {
        self.memos
            .iter()
            .filter(|m| m.has_tag_or_child(tag))
            .map(|m| m.id.clone())
            .collect()
    }

    /// Renames `old` (and every nested tag) to `new` across all memos. Returns the affected count.
    pub fn rename_tag(&mut self, old: &str, new: &str) -> Result<usize> {
        let new = new.trim().trim_start_matches('#').trim_matches('/');
        anyhow::ensure!(!new.is_empty(), t!("标签名不能为空"));
        anyhow::ensure!(
            !new.chars().any(|c| c.is_whitespace() || c == '#'),
            t!("标签名不能包含空格或 #")
        );
        if new == old {
            return Ok(0);
        }
        let ids = self.memos_with_tag(old);
        for id in &ids {
            let memo = self.get(id).unwrap();
            let content = parse::rename_tag_in(&memo.content, old, new);
            let images = memo.images.clone();
            self.update_memo(id, content, images)?;
        }
        let moved: Vec<(String, TagMeta)> = self
            .tag_meta
            .iter()
            .filter(|(t, _)| tag_matches(t, old))
            .map(|(t, m)| (format!("{new}{}", &t[old.len()..]), m.clone()))
            .collect();
        self.conn.execute(
            "DELETE FROM tag_meta WHERE tag = ?1 OR tag LIKE ?2",
            params![old, format!("{old}/%")],
        )?;
        self.tag_meta.retain(|t, _| !tag_matches(t, old));
        for (tag, meta) in moved {
            self.write_tag_meta(&tag, meta)?;
        }
        Ok(ids.len())
    }

    /// Removes the tag text from memos but keeps the memos.
    pub fn remove_tag(&mut self, tag: &str) -> Result<usize> {
        let ids = self.memos_with_tag(tag);
        for id in &ids {
            let memo = self.get(id).unwrap();
            let content = parse::remove_tag_in(&memo.content, tag);
            let images = memo.images.clone();
            self.update_memo(id, content, images)?;
        }
        self.conn.execute(
            "DELETE FROM tag_meta WHERE tag = ?1 OR tag LIKE ?2",
            params![tag, format!("{tag}/%")],
        )?;
        self.tag_meta.retain(|t, _| !tag_matches(t, tag));
        self.bump();
        Ok(ids.len())
    }

    /// Moves every memo carrying the tag into the trash.
    pub fn trash_tag_memos(&mut self, tag: &str) -> Result<usize> {
        let ids: Vec<String> = self
            .active_memos()
            .filter(|m| m.has_tag_or_child(tag))
            .map(|m| m.id.clone())
            .collect();
        for id in &ids {
            self.trash(id)?;
        }
        Ok(ids.len())
    }

    // ------------------------------------------------------------- settings

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) -> Result<()> {
        f(&mut self.settings);
        let json = serde_json::to_string(&self.settings)?;
        self.kv_set("settings", &json)?;
        self.bump();
        Ok(())
    }

    pub fn kv_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO kv(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ----------------------------------------------------------- embeddings

    pub fn load_embeddings(&self, model: &str) -> Result<HashMap<String, (String, Vec<f32>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT memo_id, hash, vector FROM embeddings WHERE model = ?1")?;
        let rows = stmt.query_map(params![model], |r| {
            let blob: Vec<u8> = r.get(2)?;
            let vector = blob
                .as_chunks::<4>().0.iter()
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, vector)))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn save_embeddings(&mut self, model: &str, items: &[(String, String, Vec<f32>)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO embeddings(memo_id, model, hash, vector) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(memo_id, model) DO UPDATE SET hash = excluded.hash, vector = excluded.vector",
            )?;
            for (id, hash, vector) in items {
                let blob: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
                stmt.execute(params![id, model, hash, blob])?;
            }
        }
        tx.commit()?;
        self.data_version = self.read_data_version();
        Ok(())
    }

    // ------------------------------------------------------------- insights

    pub fn add_insight(&mut self, record: &InsightRecord) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO insights(created_at, perspective, scope, memo_ids, content) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                record.created_at,
                record.perspective,
                record.scope,
                serde_json::to_string(&record.memo_ids)?,
                record.content
            ],
        )?;
        self.data_version = self.read_data_version();
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insights(&self) -> Result<Vec<InsightRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, perspective, scope, memo_ids, content FROM insights ORDER BY created_at DESC LIMIT 100",
        )?;
        let rows = stmt.query_map([], |r| {
            let ids: String = r.get(4)?;
            Ok(InsightRecord {
                id: r.get(0)?,
                created_at: r.get(1)?,
                perspective: r.get(2)?,
                scope: r.get(3)?,
                memo_ids: serde_json::from_str(&ids).unwrap_or_default(),
                content: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn delete_insight(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM insights WHERE id = ?1", params![id])?;
        self.data_version = self.read_data_version();
        Ok(())
    }

    // ----------------------------------------------------------------- chat

    pub fn add_chat(&mut self, role: &str, content: &str, refs: &[String]) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO chat_messages(role, content, refs, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![role, content, serde_json::to_string(refs)?, time::now()],
        )?;
        self.data_version = self.read_data_version();
        Ok(self.conn.last_insert_rowid())
    }

    pub fn chat_history(&self) -> Result<Vec<ChatRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content, refs, created_at FROM chat_messages ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let refs: String = r.get(3)?;
            Ok(ChatRecord {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                refs: serde_json::from_str(&refs).unwrap_or_default(),
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn clear_chat(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM chat_messages", [])?;
        self.data_version = self.read_data_version();
        Ok(())
    }

    // --------------------------------------------------------------- memory

    pub fn memory_profile(&self) -> MemoryProfile {
        self.kv_get("memory_profile")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_memory_profile(&mut self, profile: &MemoryProfile) -> Result<()> {
        self.kv_set("memory_profile", &serde_json::to_string(profile)?)?;
        self.data_version = self.read_data_version();
        Ok(())
    }

    // --------------------------------------------------------------- images

    pub fn save_image_bytes(&self, bytes: &[u8], extension: &str) -> Result<String> {
        let name = format!("{}.{}", new_id(), extension.trim_start_matches('.'));
        std::fs::write(self.image_path(&name), bytes)?;
        Ok(name)
    }

    pub fn import_image_file(&self, path: &Path) -> Result<String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_lowercase();
        let bytes = std::fs::read(path)?;
        self.save_image_bytes(&bytes, &ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> Store {
        let dir = std::env::temp_dir().join(format!("mori-test-{}", new_id()));
        Store::open(&dir).unwrap()
    }

    fn memo(content: &str) -> NewMemo {
        NewMemo {
            content: content.into(),
            created_at: None,
            images: vec![],
            source: "test".into(),
        }
    }

    #[test]
    fn crud_versions_trash() {
        let mut store = temp_store();
        let id = store.create_memo(memo("第一版 #测试")).unwrap();
        store
            .update_memo(&id, "第二版 #测试/子".into(), vec![])
            .unwrap();
        assert_eq!(store.get(&id).unwrap().tags, vec!["测试/子"]);
        let versions = store.versions(&id).unwrap();
        assert_eq!(versions.len(), 1);
        store.restore_version(&id, versions[0].id).unwrap();
        assert_eq!(store.get(&id).unwrap().content, "第一版 #测试");
        store.trash(&id).unwrap();
        assert_eq!(store.active_count(), 0);
        store.restore(&id).unwrap();
        assert_eq!(store.active_count(), 1);
    }

    #[test]
    fn tag_operations() {
        let mut store = temp_store();
        let a = store.create_memo(memo("A #读书/小说")).unwrap();
        let b = store.create_memo(memo("B #读书 #电影")).unwrap();
        store.set_tag_pinned("读书", true).unwrap();
        assert_eq!(store.rename_tag("读书", "阅读").unwrap(), 2);
        assert_eq!(store.get(&a).unwrap().tags, vec!["阅读/小说"]);
        assert!(store.tag_meta("阅读").pinned);
        assert_eq!(store.remove_tag("阅读").unwrap(), 2);
        assert_eq!(store.get(&b).unwrap().content, "B #电影");
    }
}
