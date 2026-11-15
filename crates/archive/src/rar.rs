//! RAR (read: RARLAB's own unpacker, built from its source, which reads
//! every RAR there is and makes none), a password asked for when one is
//! needed. Its entries go where `Sink` says, through the same checks.

use std::path::Path;

use unrar::{Archive, error::Code};

use crate::{Control, Entry, EntryKind, Error, Result, safe_path, sink::Sink};

fn rar_error(e: unrar::error::UnrarError) -> Error {
    match e.code {
        Code::MissingPassword => Error::Password(false),
        Code::BadPassword => Error::Password(true),
        Code::EOpen => Error::Bad("it can't be opened (a part of it missing?)".into()),
        _ => Error::Bad(e.to_string()),
    }
}

/// DOS's date and time (RAR's) as Unix time.
fn dos(t: u32) -> Option<i64> {
    let (date, time) = (t >> 16, t & 0xffff);
    let (y, m, d) = (1980 + (date >> 9) as i64, (date >> 5) & 0xf, date & 0x1f);
    if m == 0 || d == 0 {
        return None;
    }
    Some(crate::zipfmt::days_from_civil(y, m, d) * 86_400 + ((time >> 11) * 3600 + ((time >> 5) & 0x3f) * 60 + (time & 0x1f) * 2) as i64)
}

fn open<'a>(path: &'a Path, pw: Option<&'a str>) -> Archive<'a> {
    match pw {
        Some(p) => Archive::with_password(path, p),
        None => Archive::new(path),
    }
}

pub fn list(path: &Path, pw: Option<&str>) -> Result<Vec<Entry>> {
    let mut a = open(path, pw).open_for_listing().map_err(rar_error)?;
    let mut out = Vec::new();
    while let Some(h) = a.read_header().map_err(rar_error)? {
        let e = h.entry();
        let name = e.filename.to_string_lossy().into_owned();
        out.push(Entry {
            path: safe_path(&name),
            kind: if e.is_directory() { EntryKind::Dir } else { EntryKind::File },
            size: e.unpacked_size,
            mode: None,
            mtime: dos(e.file_time),
            encrypted: e.is_encrypted(),
            name,
        });
        a = h.skip().map_err(rar_error)?;
    }
    Ok(out)
}

pub fn extract(path: &Path, sink: &mut Sink, pw: Option<&str>, ctl: &Control) -> Result<()> {
    let entries = list(path, pw)?;
    ctl.total(entries.iter().map(|e| e.size).sum());
    let mut a = open(path, pw).open_for_processing().map_err(rar_error)?;
    while let Some(h) = a.read_header().map_err(rar_error)? {
        let e = h.entry();
        let name = e.filename.to_string_lossy().into_owned();
        let (dir, size, mtime) = (e.is_directory(), e.unpacked_size, dos(e.file_time));
        match safe_path(&name) {
            None => a = h.skip().map_err(rar_error)?,
            Some(rel) => {
                ctl.item(&name)?;
                if dir {
                    sink.dir(&rel, None, mtime)?;
                    a = h.skip().map_err(rar_error)?;
                } else {
                    let to = sink.file_path(&rel)?;
                    a = h.extract_to(&to).map_err(rar_error)?;
                    if let Some(t) = mtime {
                        crate::sink::set_mtime(&to, t);
                    }
                    ctl.advance(size)?;
                }
            }
        }
    }
    Ok(())
}
