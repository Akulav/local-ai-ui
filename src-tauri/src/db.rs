//! SQLite persistence (bundled rusqlite, WAL). Tables: projects, chats, messages (role user/assistant/
//! tool/event, JSON meta), files + chunks (RAG index with optional f32 embedding blobs), stats (tok/s).

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

use crate::state::{new_id, now_ms};

pub struct Db(pub Mutex<Connection>);

/// Column list matching `Db::row_chat`.
const CHAT_COLS: &str = "id,project_id,title,created,updated,ctx_limit,ctx_used,summary,pinned";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub instructions: String,
    pub created: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub created: i64,
    pub updated: i64,
    pub ctx_limit: u32,
    pub ctx_used: u32,
    pub summary: String,
    pub pinned: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub seq: i64,
    /// user | assistant | tool | event
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub meta: Value,
    pub compressed: bool,
    pub created: i64,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let c = Connection::open(path)?;
        c.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS projects(
                id TEXT PRIMARY KEY, name TEXT NOT NULL, folder TEXT NOT NULL,
                instructions TEXT NOT NULL DEFAULT '', created INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS chats(
                id TEXT PRIMARY KEY, project_id TEXT, title TEXT NOT NULL,
                created INTEGER NOT NULL, updated INTEGER NOT NULL,
                ctx_limit INTEGER NOT NULL DEFAULT 0, ctx_used INTEGER NOT NULL DEFAULT 0,
                summary TEXT NOT NULL DEFAULT '', pinned INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS messages(
                id TEXT PRIMARY KEY, chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
                seq INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL DEFAULT '',
                reasoning TEXT NOT NULL DEFAULT '', meta TEXT NOT NULL DEFAULT '{}',
                compressed INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS idx_msg_chat ON messages(chat_id, seq);
            CREATE TABLE IF NOT EXISTS files(
                project_id TEXT NOT NULL, path TEXT NOT NULL, mtime INTEGER NOT NULL, size INTEGER NOT NULL,
                PRIMARY KEY(project_id, path));
            CREATE TABLE IF NOT EXISTS chunks(
                id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, path TEXT NOT NULL,
                idx INTEGER NOT NULL, line_start INTEGER NOT NULL, line_end INTEGER NOT NULL,
                text TEXT NOT NULL, emb BLOB);
            CREATE INDEX IF NOT EXISTS idx_chunk_proj ON chunks(project_id, path);
            CREATE TABLE IF NOT EXISTS stats(
                id INTEGER PRIMARY KEY AUTOINCREMENT, model TEXT NOT NULL, tps REAL NOT NULL,
                tokens INTEGER NOT NULL, created INTEGER NOT NULL);
            "#,
        )?;
        // progress cards left "running" by a previous session were interrupted
        c.execute(
            "UPDATE messages SET meta=REPLACE(meta,'\"state\":\"running\"','\"state\":\"cancelled\"') WHERE role='event' AND meta LIKE '%\"state\":\"running\"%'",
            [],
        )?;
        Ok(Db(Mutex::new(c)))
    }

    // ---------- projects ----------
    pub fn create_project(&self, name: &str, folder: &str) -> Result<Project> {
        let p = Project { id: new_id(), name: name.into(), folder: folder.into(), instructions: String::new(), created: now_ms() };
        self.0.lock().execute(
            "INSERT INTO projects(id,name,folder,instructions,created) VALUES(?1,?2,?3,?4,?5)",
            params![p.id, p.name, p.folder, p.instructions, p.created],
        )?;
        Ok(p)
    }

    fn row_project(r: &rusqlite::Row) -> rusqlite::Result<Project> {
        Ok(Project { id: r.get(0)?, name: r.get(1)?, folder: r.get(2)?, instructions: r.get(3)?, created: r.get(4)? })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let c = self.0.lock();
        let mut st = c.prepare("SELECT id,name,folder,instructions,created FROM projects ORDER BY created DESC")?;
        let rows = st.query_map([], Self::row_project)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn project(&self, id: &str) -> Result<Option<Project>> {
        Ok(self
            .0
            .lock()
            .query_row("SELECT id,name,folder,instructions,created FROM projects WHERE id=?1", [id], Self::row_project)
            .optional()?)
    }

    /// The project a chat belongs to (None for standalone chats).
    pub fn chat_project(&self, chat: &Chat) -> Result<Option<Project>> {
        match &chat.project_id {
            Some(pid) => self.project(pid),
            None => Ok(None),
        }
    }

    pub fn update_project(&self, id: &str, name: &str, instructions: &str) -> Result<()> {
        self.0.lock().execute("UPDATE projects SET name=?2, instructions=?3 WHERE id=?1", params![id, name, instructions])?;
        Ok(())
    }

    pub fn delete_project(&self, id: &str) -> Result<()> {
        let mut c = self.0.lock();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM chats WHERE project_id=?1", [id])?; // messages cascade
        tx.execute("DELETE FROM chunks WHERE project_id=?1", [id])?;
        tx.execute("DELETE FROM files WHERE project_id=?1", [id])?;
        tx.execute("DELETE FROM projects WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    // ---------- chats ----------
    pub fn create_chat(&self, project_id: Option<&str>, title: &str) -> Result<Chat> {
        let t = now_ms();
        let chat = Chat {
            id: new_id(),
            project_id: project_id.map(String::from),
            title: title.into(),
            created: t,
            updated: t,
            ctx_limit: 0,
            ctx_used: 0,
            summary: String::new(),
            pinned: false,
        };
        self.0.lock().execute(
            "INSERT INTO chats(id,project_id,title,created,updated) VALUES(?1,?2,?3,?4,?5)",
            params![chat.id, chat.project_id, chat.title, chat.created, chat.updated],
        )?;
        Ok(chat)
    }

    fn row_chat(r: &rusqlite::Row) -> rusqlite::Result<Chat> {
        Ok(Chat {
            id: r.get(0)?,
            project_id: r.get(1)?,
            title: r.get(2)?,
            created: r.get(3)?,
            updated: r.get(4)?,
            ctx_limit: r.get(5)?,
            ctx_used: r.get(6)?,
            summary: r.get(7)?,
            pinned: r.get::<_, i64>(8)? != 0,
        })
    }

    /// Standalone chats (`None`) or the chats of one project, pinned first, then most recent.
    pub fn chats(&self, project_id: Option<&str>) -> Result<Vec<Chat>> {
        let c = self.0.lock();
        // `project_id IS ?1` matches NULL for standalone chats and the id otherwise
        let mut st = c.prepare(&format!("SELECT {CHAT_COLS} FROM chats WHERE project_id IS ?1 ORDER BY pinned DESC, updated DESC"))?;
        let rows = st.query_map([project_id], Self::row_chat)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn chat(&self, id: &str) -> Result<Option<Chat>> {
        Ok(self
            .0
            .lock()
            .query_row(&format!("SELECT {CHAT_COLS} FROM chats WHERE id=?1"), [id], Self::row_chat)
            .optional()?)
    }

    /// Ids of chats whose messages contain `query` (case-insensitive for ASCII), most recent first.
    pub fn search_chats(&self, query: &str) -> Result<Vec<String>> {
        let pattern = format!("%{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let c = self.0.lock();
        let mut st = c.prepare(
            "SELECT c.id FROM chats c WHERE EXISTS (SELECT 1 FROM messages m WHERE m.chat_id=c.id
               AND m.role IN ('user','assistant') AND m.content LIKE ?1 ESCAPE '\\')
             ORDER BY c.updated DESC LIMIT 200",
        )?;
        let rows = st.query_map([pattern], |r| r.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn rename_chat(&self, id: &str, title: &str) -> Result<()> {
        self.0.lock().execute("UPDATE chats SET title=?2 WHERE id=?1", params![id, title])?;
        Ok(())
    }

    pub fn pin_chat(&self, id: &str, pinned: bool) -> Result<()> {
        self.0.lock().execute("UPDATE chats SET pinned=?2 WHERE id=?1", params![id, pinned as i64])?;
        Ok(())
    }

    pub fn set_chat_ctx(&self, id: &str, used: u32) -> Result<()> {
        self.0.lock().execute("UPDATE chats SET ctx_used=?2, updated=?3 WHERE id=?1", params![id, used, now_ms()])?;
        Ok(())
    }

    /// Moves a chat into a project (or out of it with None).
    pub fn set_chat_project(&self, id: &str, project_id: Option<&str>) -> Result<()> {
        self.0.lock().execute("UPDATE chats SET project_id=?2 WHERE id=?1", params![id, project_id])?;
        Ok(())
    }

    pub fn set_chat_limit(&self, id: &str, limit: u32) -> Result<()> {
        self.0.lock().execute("UPDATE chats SET ctx_limit=?2 WHERE id=?1", params![id, limit])?;
        Ok(())
    }

    pub fn set_summary(&self, id: &str, summary: &str) -> Result<()> {
        self.0.lock().execute("UPDATE chats SET summary=?2 WHERE id=?1", params![id, summary])?;
        Ok(())
    }

    pub fn delete_chat(&self, id: &str) -> Result<()> {
        self.0.lock().execute("DELETE FROM chats WHERE id=?1", [id])?;
        Ok(())
    }

    // ---------- messages ----------
    pub fn add_message(&self, chat_id: &str, role: &str, content: &str, reasoning: &str, meta: &Value) -> Result<Message> {
        let c = self.0.lock();
        let seq: i64 = c.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM messages WHERE chat_id=?1", [chat_id], |r| r.get(0))?;
        let m = Message {
            id: new_id(),
            chat_id: chat_id.into(),
            seq,
            role: role.into(),
            content: content.into(),
            reasoning: reasoning.into(),
            meta: meta.clone(),
            compressed: false,
            created: now_ms(),
        };
        c.execute(
            "INSERT INTO messages(id,chat_id,seq,role,content,reasoning,meta,compressed,created) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8)",
            params![m.id, m.chat_id, m.seq, m.role, m.content, m.reasoning, m.meta.to_string(), m.created],
        )?;
        c.execute("UPDATE chats SET updated=?2 WHERE id=?1", params![chat_id, m.created])?;
        Ok(m)
    }

    pub fn update_message(&self, id: &str, content: &str, reasoning: &str, meta: &Value) -> Result<()> {
        self.0.lock().execute(
            "UPDATE messages SET content=?2, reasoning=?3, meta=?4 WHERE id=?1",
            params![id, content, reasoning, meta.to_string()],
        )?;
        Ok(())
    }

    pub fn messages(&self, chat_id: &str) -> Result<Vec<Message>> {
        let c = self.0.lock();
        let mut st = c.prepare(
            "SELECT id,chat_id,seq,role,content,reasoning,meta,compressed,created FROM messages WHERE chat_id=?1 ORDER BY seq",
        )?;
        let rows = st.query_map([chat_id], |r| {
            let meta: String = r.get(6)?;
            Ok(Message {
                id: r.get(0)?,
                chat_id: r.get(1)?,
                seq: r.get(2)?,
                role: r.get(3)?,
                content: r.get(4)?,
                reasoning: r.get(5)?,
                meta: serde_json::from_str(&meta).unwrap_or(Value::Null),
                compressed: r.get::<_, i64>(7)? != 0,
                created: r.get(8)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn mark_compressed(&self, chat_id: &str, upto_seq: i64) -> Result<()> {
        self.0.lock().execute(
            "UPDATE messages SET compressed=1 WHERE chat_id=?1 AND seq<?2 AND role!='event'",
            params![chat_id, upto_seq],
        )?;
        Ok(())
    }

    /// Delete a message and everything after it (used by edit / regenerate).
    pub fn truncate_from(&self, chat_id: &str, seq: i64) -> Result<()> {
        self.0.lock().execute("DELETE FROM messages WHERE chat_id=?1 AND seq>=?2", params![chat_id, seq])?;
        Ok(())
    }

    // ---------- stats ----------
    pub fn add_stat(&self, model: &str, tps: f64, tokens: u32) -> Result<()> {
        self.0.lock().execute(
            "INSERT INTO stats(model,tps,tokens,created) VALUES(?1,?2,?3,?4)",
            params![model, tps, tokens, now_ms()],
        )?;
        Ok(())
    }

    /// Token-weighted average speed of the last 50 generations of a model.
    pub fn avg_tps(&self, model: &str) -> f64 {
        self.0
            .lock()
            .query_row(
                "SELECT COALESCE(SUM(tps*tokens)/NULLIF(SUM(tokens),0),0) FROM (SELECT tps,tokens FROM stats WHERE model=?1 AND tokens>8 ORDER BY id DESC LIMIT 50)",
                [model],
                |r| r.get(0),
            )
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_moves_into_and_out_of_a_project() {
        let dir = std::env::temp_dir().join(format!("lumen-db-test-{}", crate::state::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("t.db")).unwrap();
        let p = db.create_project("Demo", dir.to_str().unwrap()).unwrap();
        let c = db.create_chat(None, "New chat").unwrap();
        assert!(db.chat_project(&db.chat(&c.id).unwrap().unwrap()).unwrap().is_none());
        db.set_chat_project(&c.id, Some(&p.id)).unwrap();
        let moved = db.chat(&c.id).unwrap().unwrap();
        assert_eq!(db.chat_project(&moved).unwrap().unwrap().id, p.id);
        assert_eq!(db.chats(Some(&p.id)).unwrap().len(), 1);
        assert!(db.chats(None).unwrap().is_empty());
        db.set_chat_project(&c.id, None).unwrap();
        assert_eq!(db.chats(None).unwrap().len(), 1);
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }
}
