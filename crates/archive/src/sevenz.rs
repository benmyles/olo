//! 7-Zip: read (its codecs; AES with a password) and written (LZMA2,
//! AES-256 and its names hidden with a password). 7-Zip keeps no
//! symbolic links: they're left out of an archive made here.

use std::{fs::File, path::Path};

use sevenz_rust2::{Archive, ArchiveEntry, ArchiveReader, ArchiveWriter, EncoderMethod, Password, encoder_options::AesEncoderOptions};

use crate::{
    Control, Entry, EntryKind, Error, Result, safe_path,
    sink::{Sink, WalkKind, Walked},
    stopped_or,
};

fn seven_error(e: sevenz_rust2::Error) -> Error {
    use sevenz_rust2::Error as E;
    match e {
        E::PasswordRequired => Error::Password(false),
        E::MaybeBadPassword(_) => Error::Password(true),
        E::Io(e, _) => Error::Io(e),
        other => {
            let s = other.to_string();
            if s.to_lowercase().contains("password") { Error::Password(true) } else { Error::Bad(s) }
        }
    }
}

fn password(p: Option<&str>) -> Password {
    p.map(Password::new).unwrap_or_else(Password::empty)
}

fn mtime(e: &ArchiveEntry) -> Option<i64> {
    e.has_last_modified_date.then(|| (u64::from(e.last_modified_date) / 10_000_000) as i64 - 11_644_473_600)
}

/// Unix permissions, when the archive kept them (in the attributes' high
/// half, as p7zip does).
fn mode(e: &ArchiveEntry) -> Option<u32> {
    (e.has_windows_attributes && e.windows_attributes & 0x8000 != 0).then_some(e.windows_attributes >> 16)
}

pub fn list(path: &Path, pw: Option<&str>) -> Result<Vec<Entry>> {
    let a = Archive::open_with_password(path, &password(pw)).map_err(seven_error)?;
    Ok(a.files
        .iter()
        .filter(|e| !e.is_anti_item)
        .map(|e| Entry {
            name: e.name.clone(),
            path: safe_path(&e.name),
            kind: if e.is_directory { EntryKind::Dir } else { EntryKind::File },
            size: e.size,
            mode: mode(e),
            mtime: mtime(e),
            encrypted: false,
        })
        .collect())
}

pub fn extract(path: &Path, sink: &mut Sink, pw: Option<&str>, ctl: &Control) -> Result<()> {
    let a = Archive::open_with_password(path, &password(pw)).map_err(seven_error)?;
    ctl.total(a.files.iter().map(|e| e.size).sum());
    let mut r = ArchiveReader::open(path, password(pw)).map_err(seven_error)?;
    let mut failed: Option<Error> = None;
    let walked = r.for_each_entries(|e, data| {
        if e.is_anti_item {
            return Ok(true);
        }
        let Some(rel) = safe_path(&e.name) else { return Ok(true) };
        if let Err(err) = ctl.item(&e.name) {
            failed = Some(err);
            return Ok(false);
        }
        let done = if e.is_directory {
            sink.dir(&rel, mode(e), mtime(e)).map_err(Error::Io)
        } else {
            let mut counted = ctl.counted(data);
            sink.file(&rel, &mut counted, mode(e), mtime(e)).map_err(|err| stopped_or(err, ctl))
        };
        match done {
            Ok(()) => Ok(true),
            Err(err) => {
                failed = Some(err);
                Ok(false)
            }
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }
    walked.map_err(seven_error)
}

pub fn compress(walked: &[Walked], partial: &Path, pw: Option<&str>, ctl: &Control) -> Result<()> {
    let mut w = ArchiveWriter::create(partial).map_err(seven_error)?;
    if let Some(p) = pw {
        w.set_content_methods(vec![AesEncoderOptions::new(Password::new(p)).into(), EncoderMethod::LZMA2.into()]);
        w.set_encrypt_header(true);
    }
    for item in walked {
        ctl.item(&item.name)?;
        let mut e = ArchiveEntry::from_path(&item.path, item.name.clone());
        // (Unix permissions in the attributes' high half, as p7zip keeps them.)
        e.has_windows_attributes = true;
        e.windows_attributes = (e.windows_attributes & 0xffff) | 0x8000 | (item.mode << 16) | if matches!(item.kind, WalkKind::Dir) { libc::S_IFDIR << 16 } else { libc::S_IFREG << 16 };
        match &item.kind {
            WalkKind::Dir => {
                w.push_archive_entry::<File>(e, None).map_err(seven_error)?;
            }
            WalkKind::File => {
                let r = ctl.counted(crate::sink::open(&item.path)?);
                w.push_archive_entry(e, Some(r)).map_err(|err| match err {
                    sevenz_rust2::Error::Io(io, _) => stopped_or(io, ctl),
                    other => seven_error(other),
                })?;
            }
            WalkKind::Symlink(_) => {}
        }
    }
    w.finish().map_err(Error::Io)?;
    Ok(())
}
