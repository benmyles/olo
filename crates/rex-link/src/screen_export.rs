//! `olo-rex-link screen export NAME [--part SPEC] [--from N] [--to N] [--zoom Z] [--encoder E]`:
//! runs the host's own `olo-screen history NAME --export`, streams progress
//! messages, then streams the resulting MP4 file in bounded binary chunks.
//!
//! Temporary export files live in a private exclusively-created `0700`
//! directory on disk (`/var/tmp/olo-screens-UID/exports/...`, never tmpfs
//! runtime memory) removed wholesale on exit. Closing stdin, sending
//! `SIGTERM`/`SIGINT`/`SIGHUP`, or breaking stdout immediately terminates the
//! entire export process group (even descendants that ignore `SIGTERM`, while
//! keeping the leader PID pinned until escalation completes) and deletes the
//! private directory.

use std::io::{self, Read as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use olo_rex_link::screen_wire::{ExportArgs, ExportMeta, ExportMsg, encode_export_msg, parse_export_args};
use serde::Deserialize;

use crate::screens::{CancelFd, OwnedFd, TrustedTempDir, disk_scratch_base, host_cli_supports_export_part, open_regular_at, pidfd_exited, pidfd_open, prefix, set_nonblocking, write_all_cancellable};

/// Default maximum exported MP4 size (2 GiB; overrideable via `OLO_SCREEN_MAX_EXPORT_BYTES`).
const DEFAULT_MAX_EXPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default minimum free disk space reserved on the export volume (256 MiB; overrideable via `OLO_SCREEN_MIN_FREE_BYTES`).
const DEFAULT_MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;
/// Poll interval while encoding to enforce output size & free-space budgets.
const BUDGET_POLL_MS: i32 = 250;

fn max_export_bytes() -> u64 {
    std::env::var("OLO_SCREEN_MAX_EXPORT_BYTES").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&n| n > 0).unwrap_or(DEFAULT_MAX_EXPORT_BYTES)
}

fn min_export_free_bytes() -> u64 {
    std::env::var("OLO_SCREEN_MIN_FREE_BYTES").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(DEFAULT_MIN_FREE_BYTES)
}

/// Measure the current size of `video.mp4` (if created) relative to the pinned
/// scratch directory fd without following symlinks.
fn export_file_size(dir_fd: &OwnedFd) -> u64 {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let name = std::ffi::CString::new("video.mp4").unwrap();
    // SAFETY: pinned directory fd and NUL-terminated single filename.
    if unsafe { libc::fstatat(dir_fd.raw(), name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG { st.st_size.max(0) as u64 } else { 0 }
}

/// Check host export byte and free-space budgets, returning an error message if
/// exceeded (Finding 11).
fn check_export_budget(scratch: &TrustedTempDir, max_bytes: u64, min_free: u64) -> Result<(), String> {
    let cur_bytes = export_file_size(scratch.dir_fd());
    if cur_bytes > max_bytes {
        return Err(format!("Export exceeded host size limit ({cur_bytes} > {max_bytes} bytes)."));
    }
    if min_free > 0
        && let Some(avail) = scratch.available_bytes()
        && avail < min_free
    {
        return Err(format!("Export stopped: host scratch disk has less than {min_free} bytes free ({avail} bytes remaining)."));
    }
    Ok(())
}

const EXPORT_CHUNK_BYTES: usize = 64 * 1024;
/// Maximum JSON stdout retained from host `olo-screen history --export`.
const MAX_EXPORT_STDOUT_BYTES: usize = 64 * 1024;
/// Maximum diagnostic stderr tail retained (well under `MAX_STATUS_PAYLOAD`).
const MAX_EXPORT_STDERR_TAIL: usize = 32 * 1024;
/// Maximum bytes kept for a single unterminated progress line.
const MAX_PROGRESS_LINE_BYTES: usize = 4096;

#[derive(Deserialize)]
struct ToolExportJson {
    #[serde(default)]
    frames: u64,
    #[serde(default)]
    pictures_encoded: u64,
    #[serde(default)]
    encoder: String,
    #[serde(default)]
    size: [u32; 2],
}

fn setup_signals() -> Option<OwnedFd> {
    // SAFETY: single-threaded process signal setup before spawning child.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGHUP);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return None;
        }
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        (fd >= 0).then(|| OwnedFd::new(fd))
    }
}

/// True if any live (non-zombie) process other than `leader_pid` still belongs
/// to process group `leader_pid`.
fn pgid_has_other_live_member(leader_pid: i32) -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else { return false };
    for entry in rd.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else { continue };
        if pid == leader_pid {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let Some((_, rest)) = stat.rsplit_once(')') else { continue };
        let mut fields = rest.split_whitespace();
        let Some(state) = fields.next() else { continue };
        if matches!(state.as_bytes().first().copied(), Some(b'Z' | b'X' | b'x')) {
            continue;
        }
        let _ppid = fields.next();
        if let Some(pgrp) = fields.next().and_then(|s| s.parse::<i32>().ok())
            && pgrp == leader_pid
        {
            return true;
        }
    }
    false
}

/// True if `child` has exited (without reaping it, keeping its PID/PGID pinned).
fn leader_has_exited(leader_pid: i32, child_pidfd: Option<&OwnedFd>) -> bool {
    if let Some(fd) = child_pidfd {
        return pidfd_exited(fd);
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{leader_pid}/stat")) else { return true };
    stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().as_bytes().first().copied()).is_some_and(|s| matches!(s, b'Z' | b'X' | b'x'))
}

/// Terminate the export child's ENTIRE process group cleanly while the leader
/// PID stays pinned (unreaped) so PGID recycling can never occur and no
/// SIGTERM-ignoring descendant can survive cancellation, then reap the leader.
fn kill_and_reap(child: &mut Child, child_pidfd: Option<&OwnedFd>) {
    let leader_pid = child.id() as i32;
    let pgid = -(leader_pid as libc::pid_t);
    // SAFETY: `child` was spawned with `.process_group(0)` and is unreaped,
    // so `-leader_pid` is guaranteed to be our export process group.
    unsafe {
        libc::kill(pgid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut group_clear = false;
    loop {
        if leader_has_exited(leader_pid, child_pidfd) && !pgid_has_other_live_member(leader_pid) {
            group_clear = true;
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !group_clear {
        // SAFETY: leader is still unreaped, so `pgid` is still pinned to our group.
        unsafe {
            libc::kill(pgid, libc::SIGKILL);
        }
        // Brief settle so any killed grandchildren exit before returning.
        let kill_wait = Instant::now() + Duration::from_millis(200);
        while pgid_has_other_live_member(leader_pid) && Instant::now() < kill_wait {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let _ = child.wait();
}

/// Extract a percentage from host `olo-screen` stderr lines such as
/// `olo-screen: exporting… 40%` or `olo-screen: exporting…40%`.
/// Scans UTF-8 bytes from the right so a multi-byte character directly before
/// the digits can never land mid-codepoint and panic (Finding 31).
fn parse_progress_line(line: &str) -> Option<f32> {
    let pct_pos = line.rfind('%')?;
    let before = line[..pct_pos].trim_end();
    let start = before.bytes().rposition(|b| !b.is_ascii_digit() && b != b'.').map_or(0, |i| i + 1);
    let val: f32 = before[start..].parse().ok()?;
    val.is_finite().then_some((val / 100.0).clamp(0.0, 1.0))
}

fn push_bounded_tail(buf: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    if chunk.len() >= cap {
        buf.clear();
        buf.extend_from_slice(&chunk[chunk.len() - cap..]);
        return;
    }
    let need = buf.len() + chunk.len();
    if need > cap {
        buf.drain(..(need - cap));
    }
    buf.extend_from_slice(chunk);
}

fn send_export(msg: &ExportMsg, cancels: &[CancelFd]) -> bool {
    let Ok((tag, payload)) = encode_export_msg(msg) else { return false };
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(tag);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    write_all_cancellable(1, &frame, cancels).is_ok()
}

fn check_cancelled(sigfd: Option<&OwnedFd>) -> bool {
    let mut pfds = Vec::with_capacity(2);
    pfds.push(libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 });
    if let Some(s) = sigfd {
        pfds.push(libc::pollfd { fd: s.raw(), events: libc::POLLIN, revents: 0 });
    }
    // SAFETY: valid fds, non-blocking poll (`timeout = 0`).
    let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, 0) };
    if n <= 0 {
        return false;
    }
    if pfds.get(1).is_some_and(|p| p.revents != 0) {
        return true;
    }
    if pfds[0].revents != 0 {
        let mut sink = [0u8; 256];
        loop {
            // SAFETY: non-blocking read from stdin to detect EOF.
            let got = unsafe { libc::read(0, sink.as_mut_ptr().cast(), sink.len()) };
            if got == 0 {
                return true;
            }
            if got < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock || err.kind() == io::ErrorKind::Interrupted {
                    break;
                }
                return true;
            }
        }
        if pfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return true;
        }
    }
    false
}

fn drain_bounded_nonblock(reader: &mut impl io::Read, dst: &mut Vec<u8>, cap: usize, keep_tail: bool) {
    let mut buf = [0u8; 4096];
    let mut total_read = 0usize;
    let max_drain = 512 * 1024;
    while total_read < max_drain {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => {
                total_read += k;
                if keep_tail {
                    push_bounded_tail(dst, &buf[..k], cap);
                } else if dst.len() < cap {
                    let take = (cap - dst.len()).min(k);
                    dst.extend_from_slice(&buf[..take]);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

pub fn run(args: &[String]) -> ! {
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        println!("usage: olo-rex-link screen export NAME [--part SPEC] [--from N] [--to N] [--zoom 1|2|3] [--encoder auto|cpu|nvidia|vaapi]");
        std::process::exit(0);
    }
    let parsed = match parse_export_args(args) {
        Ok(p) => p,
        Err(message) => {
            let _ = olo_rex_link::screen_wire::write_export_msg(&mut io::stdout().lock(), &ExportMsg::Error { message });
            std::process::exit(2);
        }
    };
    std::process::exit(export_run(&parsed))
}

fn export_run(parsed: &ExportArgs) -> i32 {
    let sigfd = setup_signals();
    set_nonblocking(0);
    set_nonblocking(1);
    let mut cancels = vec![CancelFd::StdinEof(0)];
    if let Some(s) = &sigfd {
        cancels.push(CancelFd::Readable(s.raw()));
    }

    let olo_prefix = prefix();
    let program = olo_prefix.join("bin/olo-screen");
    if !program.is_file() {
        let _ = send_export(&ExportMsg::Error { message: "Olo for screens isn't installed on this host yet.".into() }, &cancels);
        return 0;
    }
    let use_part_flag = parsed.part_ranges.len() > 1;
    if use_part_flag && !host_cli_supports_export_part(&program) {
        let _ = send_export(&ExportMsg::Error { message: "discontiguous --part selections require a newer host olo-screen (use a contiguous range)".into() }, &cancels);
        return 0;
    }

    // Place temporary export media in a hardened private `0700` directory on disk
    // (`/var/tmp/olo-screens-UID/exports/...`) pinned by directory descriptors
    // and an exclusive `.owner.lock` so crashes are reclaimed cleanly (Findings 14 & 18).
    let export_base = std::env::var_os("OLO_SCREEN_EXPORT_TMPDIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| disk_scratch_base().join("exports"));
    let scratch = match TrustedTempDir::create(&export_base, &format!("export-{}", parsed.name), "export-") {
        Ok(d) => d,
        Err(e) => {
            let _ = send_export(&ExportMsg::Error { message: format!("couldn't create export scratch folder: {e}") }, &cancels);
            return 0;
        }
    };
    let max_bytes = max_export_bytes();
    let min_free = min_export_free_bytes();
    if let Err(msg) = check_export_budget(&scratch, max_bytes, min_free) {
        let _ = send_export(&ExportMsg::Error { message: msg }, &cancels);
        return 0;
    }
    let tmp_mp4 = scratch.path().join("video.mp4");

    let mut cmd_args = vec!["history".to_owned(), parsed.name.clone(), "--export".to_owned(), tmp_mp4.display().to_string()];
    if use_part_flag && let Some(spec) = parsed.part_spec() {
        cmd_args.push("--part".into());
        cmd_args.push(spec);
    } else {
        if let Some(f) = parsed.from {
            cmd_args.push("--from".into());
            cmd_args.push(f.to_string());
        }
        if let Some(t) = parsed.to {
            cmd_args.push("--to".into());
            cmd_args.push(t.to_string());
        }
    }
    if let Some(z) = parsed.zoom {
        cmd_args.push("--zoom".into());
        cmd_args.push(z.to_string());
    }
    if let Some(ref enc) = parsed.encoder {
        cmd_args.push("--encoder".into());
        cmd_args.push(enc.clone());
    }

    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let path_env = std::env::join_paths(std::iter::once(olo_prefix.join("bin")).chain(std::env::split_paths(&path_env))).unwrap_or(path_env);

    // Spawn host `olo-screen` in its own process group with a clean unblocked
    // signal mask (`SIGTERM`/`SIGINT`/`SIGHUP` unblocked, `SIGPIPE` default) AND
    // `PR_SET_PDEATHSIG(SIGKILL)` so hard-killing this helper never orphans the
    // encoder (Finding 18 & 32).
    // SAFETY: `getpid()` has no preconditions.
    let parent_pid = unsafe { libc::getpid() };
    let mut cmd = Command::new(&program);
    cmd.args(&cmd_args).env("PATH", path_env).process_group(0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // SAFETY: `pre_exec` calls only async-signal-safe syscalls (`sigemptyset`,
    // `sigaddset`, `sigprocmask`, `signal`, `prctl`, `getppid`, `_exit`).
    unsafe {
        cmd.pre_exec(move || {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::sigaddset(&mut set, libc::SIGHUP);
            libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            if libc::getppid() != parent_pid {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = send_export(&ExportMsg::Error { message: format!("couldn't start host export: {e}") }, &cancels);
            return 0;
        }
    };
    let child_pidfd = pidfd_open(child.id() as i32);

    let Some(mut child_stdout) = child.stdout.take() else {
        kill_and_reap(&mut child, child_pidfd.as_ref());
        return 1;
    };
    let Some(mut child_stderr) = child.stderr.take() else {
        kill_and_reap(&mut child, child_pidfd.as_ref());
        return 1;
    };
    set_nonblocking(child_stdout.as_raw_fd());
    set_nonblocking(child_stderr.as_raw_fd());

    let mut stdout_bytes: Vec<u8> = Vec::new();
    let mut stderr_tail: Vec<u8> = Vec::new();
    let mut stderr_line: Vec<u8> = Vec::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;

    if !send_export(&ExportMsg::Progress { done: 0.0 }, &cancels) {
        kill_and_reap(&mut child, child_pidfd.as_ref());
        return 0;
    }
    let mut last_progress: f32 = 0.0;

    let mut buf = [0u8; 4096];
    let status = loop {
        if check_cancelled(sigfd.as_ref()) {
            kill_and_reap(&mut child, child_pidfd.as_ref());
            return 0;
        }
        if let Err(msg) = check_export_budget(&scratch, max_bytes, min_free) {
            kill_and_reap(&mut child, child_pidfd.as_ref());
            let _ = send_export(&ExportMsg::Error { message: msg }, &cancels);
            return 0;
        }
        let mut pfds = Vec::with_capacity(6);
        pfds.push(libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 });
        // Finding 17/35: also wake immediately if our stdout pipe reader closes.
        pfds.push(libc::pollfd { fd: 1, events: 0, revents: 0 });
        let out_idx = (!stdout_eof).then(|| {
            let i = pfds.len();
            pfds.push(libc::pollfd { fd: child_stdout.as_raw_fd(), events: libc::POLLIN, revents: 0 });
            i
        });
        let err_idx = (!stderr_eof).then(|| {
            let i = pfds.len();
            pfds.push(libc::pollfd { fd: child_stderr.as_raw_fd(), events: libc::POLLIN, revents: 0 });
            i
        });
        let pid_idx = child_pidfd.as_ref().map(|p| {
            let i = pfds.len();
            pfds.push(libc::pollfd { fd: p.raw(), events: libc::POLLIN, revents: 0 });
            i
        });
        let sig_idx = sigfd.as_ref().map(|s| {
            let i = pfds.len();
            pfds.push(libc::pollfd { fd: s.raw(), events: libc::POLLIN, revents: 0 });
            i
        });

        // SAFETY: `pfds` contains valid open fds for the duration of `poll`.
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, BUDGET_POLL_MS) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            kill_and_reap(&mut child, child_pidfd.as_ref());
            return 1;
        }

        if (pfds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0) || sig_idx.is_some_and(|i| pfds[i].revents != 0) || (pfds[0].revents != 0 && check_cancelled(sigfd.as_ref())) {
            kill_and_reap(&mut child, child_pidfd.as_ref());
            return 0;
        }

        if let Some(i) = out_idx
            && pfds[i].revents != 0
        {
            loop {
                match child_stdout.read(&mut buf) {
                    Ok(0) => {
                        stdout_eof = true;
                        break;
                    }
                    Ok(k) => {
                        if stdout_bytes.len() < MAX_EXPORT_STDOUT_BYTES {
                            let take = (MAX_EXPORT_STDOUT_BYTES - stdout_bytes.len()).min(k);
                            stdout_bytes.extend_from_slice(&buf[..take]);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted => break,
                    Err(_) => {
                        stdout_eof = true;
                        break;
                    }
                }
            }
        }

        if let Some(i) = err_idx
            && pfds[i].revents != 0
        {
            loop {
                match child_stderr.read(&mut buf) {
                    Ok(0) => {
                        stderr_eof = true;
                        break;
                    }
                    Ok(k) => {
                        push_bounded_tail(&mut stderr_tail, &buf[..k], MAX_EXPORT_STDERR_TAIL);
                        push_bounded_tail(&mut stderr_line, &buf[..k], MAX_PROGRESS_LINE_BYTES);
                        while let Some(sep) = memchr::memchr2(b'\n', b'\r', &stderr_line) {
                            let line = String::from_utf8_lossy(&stderr_line[..sep]).into_owned();
                            stderr_line.drain(..=sep);
                            if let Some(done) = parse_progress_line(&line)
                                && (done - last_progress).abs() >= 0.005
                            {
                                last_progress = done;
                                if !send_export(&ExportMsg::Progress { done }, &cancels) {
                                    kill_and_reap(&mut child, child_pidfd.as_ref());
                                    return 0;
                                }
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted => break,
                    Err(_) => {
                        stderr_eof = true;
                        break;
                    }
                }
            }
        }

        let exited = pid_idx.is_some_and(|i| pfds[i].revents != 0) || (stdout_eof && stderr_eof);
        if exited && let Ok(Some(st)) = child.try_wait() {
            // Bounded non-blocking drain of any remaining buffered stdout/stderr.
            drain_bounded_nonblock(&mut child_stdout, &mut stdout_bytes, MAX_EXPORT_STDOUT_BYTES, false);
            drain_bounded_nonblock(&mut child_stderr, &mut stderr_tail, MAX_EXPORT_STDERR_TAIL, true);
            break st;
        }
    };

    if !status.success() {
        let err_text = String::from_utf8_lossy(&stderr_tail).trim().to_owned();
        let message = if err_text.is_empty() { format!("export exited with status {}", status.code().unwrap_or(1)) } else { err_text };
        let _ = send_export(&ExportMsg::Error { message }, &cancels);
        return 0;
    }

    let Ok((mut mp4_file, mp4_meta)) = open_regular_at(scratch.dir_fd(), "video.mp4") else {
        let _ = send_export(&ExportMsg::Error { message: "exported video file missing".into() }, &cancels);
        return 0;
    };
    let file_bytes = mp4_meta.len();
    if file_bytes > max_bytes {
        let _ = send_export(&ExportMsg::Error { message: format!("Export exceeded host size limit ({file_bytes} > {max_bytes} bytes).") }, &cancels);
        return 0;
    }
    let parsed_json: ToolExportJson = serde_json::from_slice(&stdout_bytes).unwrap_or(ToolExportJson { frames: 0, pictures_encoded: 0, encoder: String::new(), size: [0, 0] });

    if last_progress < 1.0 && !send_export(&ExportMsg::Progress { done: 1.0 }, &cancels) {
        return 0;
    }

    let meta = ExportMeta { bytes: file_bytes, frames: parsed_json.frames, encoded: parsed_json.pictures_encoded, encoder: parsed_json.encoder, size: (parsed_json.size[0], parsed_json.size[1]) };
    if !send_export(&ExportMsg::Meta(meta), &cancels) {
        return 0;
    }

    let mut chunk_buf = vec![0u8; EXPORT_CHUNK_BYTES];
    loop {
        if check_cancelled(sigfd.as_ref()) {
            return 0;
        }
        match mp4_file.read(&mut chunk_buf) {
            Ok(0) => break,
            Ok(n) => {
                if !send_export(&ExportMsg::Chunk(chunk_buf[..n].to_vec()), &cancels) {
                    return 0;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = send_export(&ExportMsg::Error { message: format!("reading exported video: {e}") }, &cancels);
                return 0;
            }
        }
    }

    let _ = send_export(&ExportMsg::Done, &cancels);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_rex_link::screen_wire::parse_part_spec;

    #[test]
    fn parses_part_specs_and_progress_lines() {
        assert_eq!(parse_part_spec("0-4").unwrap(), (1, 5));
        assert_eq!(parse_part_spec("2,3,4").unwrap(), (3, 5));
        assert_eq!(parse_part_spec("3..=6").unwrap(), (4, 7));
        assert!(parse_part_spec("").is_err());
        assert!(parse_part_spec("abc").is_err());

        assert_eq!(parse_progress_line("olo-screen: exporting… 40%"), Some(0.4));
        assert_eq!(parse_progress_line("olo-screen: exporting… 100%"), Some(1.0));
        assert_eq!(parse_progress_line("something unrelated"), None);
    }

    #[test]
    fn validates_export_cli_arguments() {
        let ok = parse_export_args(&["qa".into(), "--part".into(), "1-5".into(), "--zoom".into(), "2".into()]).unwrap();
        assert_eq!((ok.name.as_str(), ok.from, ok.to, ok.zoom), ("qa", Some(2), Some(6), Some(2)));
        assert!(parse_export_args(&["bad/name".into()]).is_err());
        assert!(parse_export_args(&["--clear".into()]).is_err());
        assert!(parse_export_args(&["qa".into(), "--zoom".into(), "9".into()]).is_err());
        assert!(parse_export_args(&["qa".into(), "--bogus".into()]).is_err());
    }

    #[test]
    fn review_30_31_discontiguous_part_and_non_ascii_progress() {
        // Finding 30: discontiguous selection must be rejected instead of silently widened.
        assert!(parse_part_spec("2,5,7").is_err(), "discontiguous --part must not widen to 3..=8");
        assert!(parse_part_spec("0,2-3,7").is_err(), "discontiguous --part must be rejected");
        assert_eq!(parse_part_spec("2,3,4").unwrap(), (3, 5));
        assert_eq!(parse_part_spec("4,2-3").unwrap(), (3, 5));

        // Finding 31: non-ASCII character directly before digits must NOT panic.
        assert_eq!(parse_progress_line("olo-screen: exporting…40%"), Some(0.4));
        assert_eq!(parse_progress_line("進捗75.5%"), Some(0.755));
    }
}
