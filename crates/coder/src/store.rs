//! SQLite WAL: snapshots and their audit event commit atomically. History is paged,
//! never loaded wholesale on reconnect. Each effect is journaled before dispatch.
use crate::model::*;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
};

pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join("sessions.sqlite3");
        let db = Connection::open(&path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(version <= 4, "session database was written by a newer Olo Coder");
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, updated INTEGER NOT NULL, body TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT, session TEXT NOT NULL, time INTEGER NOT NULL, kind TEXT NOT NULL, data TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS event_session ON events(session,seq);
          CREATE TABLE IF NOT EXISTS edits(id INTEGER PRIMARY KEY AUTOINCREMENT, session TEXT NOT NULL, path TEXT NOT NULL, before BLOB, after BLOB NOT NULL, undone INTEGER NOT NULL DEFAULT 0);
          CREATE VIRTUAL TABLE IF NOT EXISTS event_search USING fts5(data,content='events',content_rowid='seq',tokenize='trigram');
          CREATE TRIGGER IF NOT EXISTS event_search_insert AFTER INSERT ON events BEGIN INSERT INTO event_search(rowid,data) VALUES(new.seq,new.data); END;",
        )?;
        if version < 2 {
            db.execute_batch("BEGIN IMMEDIATE; INSERT INTO event_search(event_search) VALUES('rebuild'); PRAGMA user_version=2; COMMIT;")?;
        }
        if version < 3 {
            db.execute_batch("BEGIN IMMEDIATE; ALTER TABLE edits ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0; PRAGMA user_version=3; COMMIT;")?;
        }
        if version < 4 {
            db.execute_batch("BEGIN IMMEDIATE;
              CREATE TABLE IF NOT EXISTS messages(seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, sender TEXT, recipient TEXT NOT NULL, body TEXT NOT NULL, original TEXT NOT NULL, state TEXT NOT NULL, updated INTEGER NOT NULL);
              CREATE INDEX IF NOT EXISTS message_recipient ON messages(recipient,seq);
              CREATE INDEX IF NOT EXISTS message_sender ON messages(sender,seq);
              CREATE INDEX IF NOT EXISTS session_pin ON sessions(coalesce(json_extract(body,'$.pinned'),0) DESC,updated DESC,id);
              PRAGMA user_version=4; COMMIT;")?;
        }
        Ok(Self { db })
    }
    pub fn get(&self, id: &str) -> Result<Session> {
        let body: String = self.db.query_row("SELECT body FROM sessions WHERE id=?", [id], |r| r.get(0)).context("session not found")?;
        Ok(serde_json::from_str(&body)?)
    }
    pub fn list(&self) -> Result<Vec<Value>> {
        let mut q = self.db.prepare("SELECT json_object('id',id,'title',json_extract(body,'$.title'),'workspace',json_extract(body,'$.workspace'),'status',json_extract(body,'$.status'),'updated',updated,'parent',json_extract(body,'$.parent'),'model',json_extract(body,'$.config.model.model'),'usage',json_extract(body,'$.usage')) FROM sessions ORDER BY updated DESC")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn browse(&self, filter: &SessionFilter) -> Result<Value> {
        anyhow::ensure!(filter.offset <= i64::MAX as usize - 500 && filter.query.len() <= 4096, "catalog offset or query is too large");
        let limit = if filter.limit == 0 { 100 } else { filter.limit.min(500) };
        let mut q = self.db.prepare("SELECT json_object('id',id,'title',json_extract(body,'$.title'),'workspace',json_extract(body,'$.workspace'),
          'status',json_extract(body,'$.status'),'updated',updated,'parent',json_extract(body,'$.parent'),'model',json_extract(body,'$.config.model.model'),
          'pinned',json(CASE WHEN json_extract(body,'$.pinned') THEN 'true' ELSE 'false' END),'archived',json(CASE WHEN json_extract(body,'$.archived') THEN 'true' ELSE 'false' END),
          'tags',json(coalesce(json_extract(body,'$.tags'),'[]')),'pending',coalesce(json_array_length(body,'$.inbox'),0)) FROM sessions
          WHERE (? OR NOT coalesce(json_extract(body,'$.archived'),0)) AND (?='' OR instr(lower(json_extract(body,'$.title') || ' ' || json_extract(body,'$.workspace') || ' ' || coalesce(json_extract(body,'$.tags'),'')),lower(?))>0)
          ORDER BY coalesce(json_extract(body,'$.pinned'),0) DESC,updated DESC,id LIMIT ? OFFSET ?")?;
        let rows = q.query_map(params![filter.archived, filter.query, filter.query, (limit + 1) as i64, filter.offset as i64], |r| r.get::<_, String>(0))?;
        let mut rows = rows.map(|r| Ok(serde_json::from_str::<Value>(&r?)?)).collect::<Result<Vec<_>>>()?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        Ok(serde_json::json!({"sessions":rows,"next_offset":more.then_some(filter.offset+limit)}))
    }
    pub fn unconfirmed_messages(&self) -> Result<Vec<(String, String)>> {
        let mut q = self.db.prepare("SELECT sender,id FROM messages WHERE state='sending'")?;
        Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn original_message(&self, id: &str) -> Result<Option<Message>> {
        let row = self.db.query_row("SELECT original FROM messages WHERE id=?", [id], |r| r.get::<_, String>(0)).optional()?;
        row.map(|row| Ok(serde_json::from_str(&row)?)).transpose()
    }
    pub fn receipt(&self, id: &str) -> Result<Option<Value>> {
        let row = self.db.query_row("SELECT json_object('sequence',seq,'message',json(body),'state',state,'updated',updated) FROM messages WHERE id=?", [id], |r| r.get::<_, String>(0)).optional()?;
        row.map(|r| Ok(serde_json::from_str(&r)?)).transpose()
    }
    pub fn messages(&self, session: &str, after: i64, limit: usize) -> Result<Vec<Value>> {
        let mut q =
            self.db.prepare("SELECT json_object('sequence',seq,'message',json(body),'state',state,'updated',updated) FROM messages WHERE (recipient=? OR sender=?) AND seq>? ORDER BY seq LIMIT ?")?;
        let rows = q.query_map(params![session, session, after, limit.clamp(1, 500) as i64], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn tail_cursor(&self, session: &str, count: i64) -> Result<i64> {
        Ok(self.db.query_row("SELECT coalesce((SELECT seq FROM events WHERE session=? ORDER BY seq DESC LIMIT 1 OFFSET ?),0)", params![session, count], |r| r.get(0))?)
    }
    pub fn save(&mut self, s: &mut Session, kind: &str, data: Value) -> Result<Event> {
        s.updated = now();
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO sessions VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET updated=excluded.updated,body=excluded.body", params![s.id, s.updated as i64, serde_json::to_string(s)?])?;
        match kind {
            "message_queued" | "message_outgoing" => {
                let message: Message = serde_json::from_value(data["message"].clone())?;
                tx.execute(
                    "INSERT INTO messages(id,sender,recipient,body,original,state,updated) VALUES(?,?,?,?,?,?,?)",
                    params![
                        message.id,
                        message.from.as_ref().map(|s| &s.session),
                        message.to,
                        serde_json::to_string(&message)?,
                        serde_json::to_string(&message)?,
                        if kind == "message_queued" { "queued" } else { "sending" },
                        now() as i64
                    ],
                )?;
            }
            "message_edited" => {
                let message = &data["message"];
                tx.execute("UPDATE messages SET body=?,updated=? WHERE id=?", params![message.to_string(), now() as i64, message["id"].as_str()])?;
            }
            "messages_delivered" => {
                for id in data["ids"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    tx.execute("UPDATE messages SET state='delivered',updated=? WHERE id=?", params![now() as i64, id])?;
                }
            }
            "message_cancelled" | "message_sent" | "message_uncertain" => {
                let state = match kind {
                    "message_cancelled" => "cancelled",
                    "message_sent" => "sent",
                    _ => "uncertain",
                };
                tx.execute("UPDATE messages SET state=?,updated=? WHERE id=? AND NOT(state='sent' AND ?='uncertain')", params![state, now() as i64, data["id"].as_str(), state])?;
            }
            _ => {}
        }
        tx.execute("INSERT INTO events(session,time,kind,data) VALUES(?,?,?,?)", params![s.id, now() as i64, kind, data.to_string()])?;
        let seq = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Event { seq, session: s.id.clone(), time: now(), kind: kind.into(), data })
    }
    pub fn events(&self, session: &str, after: i64, limit: usize) -> Result<Vec<Event>> {
        let mut q = self.db.prepare("SELECT seq,time,kind,data FROM events WHERE session=? AND seq>? ORDER BY seq LIMIT ?")?;
        let rows = q.query_map(params![session, after, limit.clamp(1, 1000) as i64], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get::<_, String>(3)?)))?;
        rows.map(|r| {
            let (seq, time, kind, data) = r?;
            Ok(Event { seq, session: session.into(), time, kind, data: serde_json::from_str(&data)? })
        })
        .collect()
    }
    pub fn search(&self, session: &str, query: &str) -> Result<Vec<Event>> {
        // Trigrams preserve literal substring search while avoiding a scan of
        // weeks of tool output. Very short queries have no trigram to look up.
        let indexed = query.chars().count() >= 3;
        let needle = if indexed { format!("\"{}\"", query.replace('"', "\"\"")) } else { query.to_owned() };
        let sql = if indexed {
            "SELECT e.seq,e.time,e.kind,e.data FROM event_search JOIN events e ON e.seq=event_search.rowid WHERE e.session=? AND event_search MATCH ? ORDER BY e.seq DESC LIMIT 100"
        } else {
            "SELECT seq,time,kind,data FROM events WHERE session=? AND instr(lower(data),lower(?))>0 ORDER BY seq DESC LIMIT 100"
        };
        let mut q = self.db.prepare(sql)?;
        let rows = q.query_map(params![session, needle], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get::<_, String>(3)?)))?;
        rows.map(|r| {
            let (seq, time, kind, data) = r?;
            Ok(Event { seq, session: session.into(), time, kind, data: serde_json::from_str(&data)? })
        })
        .collect()
    }
    pub fn edit(&self, session: &str, path: &Path, before: Option<&[u8]>, after: &[u8]) -> Result<i64> {
        self.db.execute("INSERT INTO edits(session,path,before,after) VALUES(?,?,?,?)", params![session, path.to_string_lossy(), before, after])?;
        Ok(self.db.last_insert_rowid())
    }
    pub fn deleted_edit(&self, session: &str, path: &Path, before: &[u8]) -> Result<i64> {
        self.db.execute("INSERT INTO edits(session,path,before,after,deleted) VALUES(?,?,?,X'',1)", params![session, path.to_string_lossy(), before])?;
        Ok(self.db.last_insert_rowid())
    }
    pub fn checkpoints(&self, session: &str) -> Result<Value> {
        let mut q = self.db.prepare("SELECT id,path,length(before),length(after),undone,deleted FROM edits WHERE session=? ORDER BY id DESC LIMIT 500")?;
        let rows=q.query_map([session],|r|Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"path":r.get::<_,String>(1)?,"before_bytes":r.get::<_,Option<i64>>(2)?,"after_bytes":r.get::<_,i64>(3)?,"undone":r.get::<_,bool>(4)?,"deleted":r.get::<_,bool>(5)?})))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into())
    }
    pub fn undo(&self, session: &str, edit: i64) -> Result<String> {
        let (path, before, after, undone, deleted): (String, Option<Vec<u8>>, Vec<u8>, bool, bool) =
            self.db
                .query_row("SELECT path,before,after,undone,deleted FROM edits WHERE id=? AND session=?", params![edit, session], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        anyhow::ensure!(!undone, "edit already undone");
        crate::tools::path(&self.get(session)?.workspace, &path, deleted)?;
        if deleted {
            anyhow::ensure!(matches!(std::fs::symlink_metadata(&path),Err(e) if e.kind()==std::io::ErrorKind::NotFound), "path was recreated after deletion; refusing to overwrite it");
        } else {
            anyhow::ensure!(std::fs::read(&path)? == after, "file changed after this edit; refusing to overwrite newer work");
        }
        if let Some(before) = before {
            crate::tools::atomic_write(Path::new(&path), &before)?;
        } else {
            std::fs::remove_file(&path)?;
        }
        self.db.execute("UPDATE edits SET undone=1 WHERE id=?", [edit])?;
        Ok(path)
    }
}
