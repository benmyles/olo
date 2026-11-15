//! Zip: read (stored, deflate, deflate64, bzip2, zstd, LZMA; ZipCrypto
//! and AES with a password) and written (deflate, AES-256 with a
//! password); Unix permissions, times and symbolic links kept.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use zip::{AesMode, CompressionMethod, DateTime, ZipArchive, ZipWriter, result::ZipError, write::SimpleFileOptions};

use crate::{
    Control, Entry, EntryKind, Error, Result, safe_path,
    sink::{Sink, WalkKind, Walked},
    stopped_or,
};

/// Days from 1970 to a date (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn unix_of(t: DateTime) -> i64 {
    days_from_civil(t.year() as i64, t.month() as u32, t.day() as u32) * 86_400 + t.hour() as i64 * 3600 + t.minute() as i64 * 60 + t.second() as i64
}

fn datetime_of(t: i64) -> DateTime {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // (days back to a date: as in the civil algorithm.)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as u16;
    DateTime::from_date_and_time(y, m, d, (secs / 3600) as u8, (secs % 3600 / 60) as u8, (secs % 60) as u8).unwrap_or_default()
}

fn zip_error(e: ZipError) -> Error {
    match e {
        ZipError::InvalidPassword => Error::Password(true),
        ZipError::UnsupportedArchive(s) if s.to_lowercase().contains("password") => Error::Password(false),
        ZipError::UnsupportedArchive(s) => Error::Unsupported(format!("This zip uses something Olo can't read: {s}")),
        ZipError::Io(e) => Error::Io(e),
        other => Error::Bad(other.to_string()),
    }
}

pub fn list(path: &Path) -> Result<Vec<Entry>> {
    let mut z = ZipArchive::new(File::open(path)?).map_err(zip_error)?;
    let mut out = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(zip_error)?;
        let kind = if f.is_dir() {
            EntryKind::Dir
        } else if f.is_symlink() {
            EntryKind::Symlink(String::new())
        } else {
            EntryKind::File
        };
        let name = f.name().map(|n| n.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(f.name_raw()).into_owned());
        out.push(Entry { path: safe_path(&name), name, kind, size: f.size(), mode: f.unix_mode(), mtime: f.last_modified().map(unix_of), encrypted: f.encrypted() });
    }
    Ok(out)
}

pub fn extract(path: &Path, sink: &mut Sink, password: Option<&str>, ctl: &Control) -> Result<()> {
    let mut z = ZipArchive::new(File::open(path)?).map_err(zip_error)?;
    let total: u64 = (0..z.len()).filter_map(|i| z.by_index_raw(i).ok().map(|f| f.size())).sum();
    ctl.total(total);
    for i in 0..z.len() {
        let f = match password {
            Some(pw) => z.by_index_decrypt(i, pw.as_bytes()),
            None => z.by_index(i),
        };
        let mut f = f.map_err(zip_error)?;
        let name = f.name().map(|n| n.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(f.name_raw()).into_owned());
        let Some(rel) = safe_path(&name) else { continue };
        ctl.item(&name)?;
        let mode = f.unix_mode();
        let mtime = f.last_modified().map(unix_of);
        if f.is_dir() {
            sink.dir(&rel, mode, mtime)?;
        } else if f.is_symlink() {
            let mut target = String::new();
            f.read_to_string(&mut target)?;
            sink.symlink(&rel, &target);
        } else {
            let mut r = ctl.counted(&mut f);
            sink.file(&rel, &mut r, mode, mtime).map_err(|e| match zip_io(&e) {
                Some(err) => err,
                None => stopped_or(e, ctl),
            })?;
        }
    }
    Ok(())
}

/// A wrong password shows only as the data failing its check.
fn zip_io(e: &io::Error) -> Option<Error> {
    let s = e.to_string().to_lowercase();
    (s.contains("password") || s.contains("invalid checksum") || s.contains("hmac")).then_some(Error::Password(true))
}

pub fn compress(walked: &[Walked], partial: &Path, password: Option<&str>, ctl: &Control) -> Result<()> {
    let file = File::create_new(partial)?;
    let mut z = ZipWriter::new(io::BufWriter::new(file));
    for w in walked {
        ctl.item(&w.name)?;
        let mut o =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).unix_permissions(w.mode).last_modified_time(datetime_of(w.mtime)).large_file(w.size >= u32::MAX as u64);
        if let Some(pw) = password {
            o = o.with_aes_encryption(AesMode::Aes256, pw);
        }
        match &w.kind {
            WalkKind::Dir => z.add_directory(format!("{}/", w.name), o).map_err(zip_error)?,
            WalkKind::Symlink(target) => z.add_symlink(&w.name, target, o).map_err(zip_error)?,
            WalkKind::File => {
                z.start_file(&w.name, o).map_err(zip_error)?;
                let mut r = ctl.counted(crate::sink::open(&w.path)?);
                io::copy(&mut r, &mut z).map_err(|e| stopped_or(e, ctl))?;
            }
        }
    }
    z.finish().map_err(zip_error)?;
    Ok(())
}
