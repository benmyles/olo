# Ignored tests

The workspace's tests marked `#[ignore]`: the ones that read or change
this machine, time something, need sample files, or make fixtures. Each
row says how to run it without touching the person's session, and how
it went last (2026-11-08, release builds on the ThinkPad P1 Gen 9,
after the whole workspace's 1,546 other tests passed). Run one with
`cargo test --release -p CRATE [--lib|--test T] -- --ignored --nocapture
--exact PATH`, with what its row says set.

Ways in that keep the person's session out of it:

- **Private keyring** (`tools/dev/private-keyring start`, a fresh one
  per group: tests leave items behind): a bus of its own with its own
  GNOME Keyring, never theirs.
- **PipeWire sandbox** (`tools/dev/pw-sandbox DIR start`, then the
  built test binary run under its `env`: it sets HOME, so cargo can't
  run inside it): a sound server of its own, nothing of theirs heard or
  changed.
- **Stand-ins**: `crates/system/examples/fake-readers.rs` for fprintd
  and olo-faced on a private bus (`OLO_FPRINT_BUS`, debug builds); a
  PAM service of the test's own (`OLO_PAM_CONFDIR`/`OLO_PAM_SERVICE`,
  debug builds).
- **Agent screens** (`olo-screen`), for a program that needs the
  accessibility bus; programs given their own HOME and config folders
  with `/usr/bin/env`.
- **Samples**: made with the ffmpeg Fedora ships (nothing installed),
  as each test's comment says, or public test files (Chromium's media
  test data, libheif's examples, Wikimedia Commons), each kept in a
  folder of its own under /var/tmp and removed after.

## app_server

| Test | How | 2026-11-08 |
|---|---|---|
| `color::tests::gpu_cost` | as is | pass: 0.596 ms plain, 0.735 P3, 1.543 HDR (HACKING: 0.618, 0.705, 1.53) |
| `headless::gpu_timing::gpu_upload_timing` | as is | pass: a full frame's commands 0.708 ms median |
| `rescue::tests::rescue_cost` | as is | pass: 1.47 ns a key, 0.32 ns the check (HACKING: 1.4, 0.32) |
| `shell::deco::title_tests::tab_timing` | as is | pass: 0.03–0.17 ms a tab drawn |
| `tiles::tests::live` | as is (opens /dev/dri cards: wakes NVIDIA's) | pass: the Studio Display's tiles read |
| `touch_filter::tests::frame_cost` | as is | pass: 25.7 µs a frame, Olo's model |
| `touch_filter::tests::replay_capture` | `OLO_TOUCH_CAPTURE`: the repo's `thinkpad-2026-11-05-fingers.json` written out as raw `input_event`s (as `thinkpad_frame_cost` frames it) | pass: 12,360 frames, none too big, libinput's touches always the filter's |
| `touch_filter::tests::thinkpad_frame_cost` | as is | pass: 7.62 µs a frame |
| `touch_health::tests::note_cost` | as is | pass: 21.7 ns a report (15 on a P core); the minute's figures 44–68 µs (HACKING corrected) |
| `udev::tests::left_asleep_here` | as is (sysfs) | pass: card1 left asleep |
| `wallpaper::tests::wallpaper_cost` | as is | pass: SDR 70 ms, HDR 90 ms (HACKING: 73, 94) |

## The kits, crash, DiskBackup, Doctor

| Test | How | 2026-11-08 |
|---|---|---|
| olo-interface `bounds_tree::tests::bounds_tree_cards` | as is | pass: 0.164 ms a frame |
| olo-interface `…text_system::tests::shaping_cost` | as is | pass: 2.34 µs a line |
| olo-interface `text_system::olo_shaping_speed::shaping_speed` | as is | pass: 140 ns a character |
| olo-crash `install_cost` | as is | pass: 18 µs median (HACKING: 18) |
| olo-diskbackup `throughput` | as is (backs up `crates/` into /var/tmp) | pass: 138 MB/s first, unchanged again |
| olo-diskbackup-app `serve_google` | the fake Google it serves (`OLO_FAKE_GOOGLE_AT`); the build's DiskBackup on an agent screen with its own HOME and config, `OLO_DISKBACKUP_DIR`, `_KEY`, `_GOOGLE_API`, `_GOOGLE`; the browser played as `set_up_drive` plays it | pass: the setup window from the client's ID through the sign-in to "Backed up … 2 snapshots kept" in the fake Drive |
| olo-doctor `fix::live::rpm_fusion_s_release_package_is_there` | network | pass |
| olo-doctor `releases::tests::installs_from_github` | network; two binaries into /var/tmp, removed | pass |

## Media: olo-images, olo-preview, olo-video, olo-eld, olo-face

| Test | How | 2026-11-08 |
|---|---|---|
| olo-images `tests::hdr_cost`, `tests::p3_cost` | as is | pass: 4K HDR 18.6 ms a picture; P3 32.2 ms |
| olo-images `tests::hdr_pattern` | `OLO_HDR_OUT` a scratch folder | pass |
| olo-images `tests::samples` | `OLO_SAMPLES`: libheif's `examples/example.avif`, `.heic` | pass |
| olo-images `webm::tests::plays_vp9_with_alpha` | Chromium's `bear-vp9a.webm` | pass: 82 frames, 1.34 ms each, alpha 68 |
| olo-preview `opus::tests::opus_file` | an Ogg Opus file over 5 s (ffmpeg's libopus; and a Commons file with a VP8 stream first) | pass (after f9536320: the track as the Viewer takes it) |
| olo-preview `video::tests::av1_in_mp4`, `vp9_plays`, `hevc_plays`, `b_frames`, `seeks_to_the_frame`, `keeps_up`, `seek_cost` | the samples their comments name, made as they say: testsrc2 through SVT-AV1, libvpx (`-auto-alt-ref 1 -lag-in-frames 25`), NVENC (H.264 with and without B-frames, 1080p HEVC as hev1, hvc1 and Matroska), the JCT-VC Main 10 stream remuxed from zero, 4K H.264 | pass: every picture, every seek; keeps_up 235–2,103 pictures a second; seek 267 ms (HACKING: 317). The originals were made on the Mac and kept nowhere here; clips made as the comments say count (Ben, 2026-11-08) |
| olo-preview `video::tests::hdr10_in_the_stream` | Chromium's `bear-1280x720-hevc-10bit-hdr10.mp4` | pass |
| olo-preview `video::tests::plays_across_another_encoders_entry` | `OLO_FALLBACK_MP4`: olo-video's `a_recording_goes_on_with_another_encoder` kept (`OLO_KEEP_TEST_VIDEO=1`) | pass |
| olo-video `export::tests::make_4k60`, `make_fixture`, `make_4k60_y4m` | `OLO_FIXTURE_DIR`, `OLO_FIXTURE`; `OLO_Y4M` a FIFO ffmpeg's libvpx reads | pass; the 4K60 clips played by the Viewer headless to their end on Intel's GPU, nothing dropped (H.264, HEVC, AV1, VP9) |
| olo-video `tests::encode_cost`, `encoder_opening`, `nvenc_cost_by_codec`, `nvenc_makes_h264_that_decodes`, `nvenc_makes_hevc_and_av1_mp4s` | as is (wakes NVIDIA's GPU) | pass: NVENC 4K 12–15 ms a frame, every picture decoded back |
| olo-video `tests::codec_sizes`, `tests::vaapi_sizes` | `OLO_BENCH_SCREEN` a screen-like PNG | pass |
| olo-eld `decodes_a_capture_as_libfdk_does` | a speech stream libfdk's encoder made with AirPods' config (f8e63000, ctypes on libfdk-aac.so.2) and libfdk's decode of it | pass: 709 frames, SNR 82.6 dB (50 wanted) |
| olo-face `model::tests::recognises_a_face_in_grey` | `OLO_FACE_SAMPLES`: the installed models and four NASA portraits (Commons, public domain) | pass: 0.592 alike, 0.09–0.26 apart |
| olo-face `emitter::tests::this_machines_units` | opens every /dev/video node | **not run: the person's cameras** (Blocked) |

## Palm, Palette, Passwords, Screenshot, Terminal, kit, Voice, Viewer, xdg

| Test | How | 2026-11-08 |
|---|---|---|
| olo-palm `kind::tests::make_thinkpad_model` | as is (rewrites `kind/thinkpad_model.rs`: restore it) | pass: the committed model again, to the last digit |
| olo-palm `captures::on_real_hands`, `what_the_runs_dont_show`, `make_default_model` | `OLO_PALM_CAPTURES`: the MacBook's runs | **not run: the captures are on the Mac** (Blocked); a ThinkPad run is left out now (138f0c16) |
| olo-palette `model::tests::tune_cost`, `sources::tests::ranking_cost`, `ranking_cost_with_the_model` | as is | pass: 5.8 ms a tune; 102 µs a ranking |
| olo-passwords `vault::tests::fixture`, `ssh_fixture` | `OLO_FIXTURE_VAULT`, `OLO_FIXTURE_KEY` in /var/tmp | pass |
| olo-passwords `vault::tests::through_a_backup` | private keyring; `XDG_CONFIG_HOME` in /var/tmp; `OLO_BACKUP_TEST=make`, then `check` | pass |
| olo-screenshot `inset::timing::draw_per_frame` | as is | pass: 0.147 ms a frame |
| olo-terminal, olo-termd `io::tests::scan_cost`; olo-terminal `tile_group_tests::tile_rows_cost` | as is | pass: 1.5 GB/s scanned; 186 ns |
| olo-kit `widgets::image_view::resample_timing::filters` | as is | pass |
| olo-kit-base `spell::tests::check_cost` | as is (this machine's Hunspell dictionary) | pass: 20,000 words in 3.81–3.91 ms |
| olo-voice `tests::allocations_a_frame`, `tests::live_frame_time`; `least_delay_against_deepfilternet` | as is | pass: 245 µs a frame median |
| olo-viewer `pdfdoc::tests::write_fixture`, `dump_fields`, `render_png`, `pdftext::live::reads_a_real_pdf`, `pdfdoc::sample::sample_pdf` | `OUT`, `PDF`, `OLO_PDF`, `OLO_SAMPLE_PDF` in /var/tmp: the fixture made, then read, drawn and its text read | pass |
| olo-logs `journal::tests::reads_the_real_journal`, `tests::every_real_line_highlights` | as is (reads the journal) | pass: 90,198 entries |
| olo-xdg `open::live::handlers`, `open::live::for_others`, `scope::tests::a_real_scope` | as is (a 3 s scope in the person's systemd, gone with it) | pass |
| olo-screen `atspi::tests::live` | on an agent screen (`olo-screen run … /usr/bin/env OLO_ATSPI_PID=… TEST`), a GTK program on it | pass: GNOME Text Editor's tree, 30 nodes, 32 ms |

## Keyrings, secrets, Token Pool, session

| Test | How | 2026-11-08 |
|---|---|---|
| olo-system `secrets::tests::round_trip`, `github::tests::tokens_from_a_keyring`, `keyring_prompt::tests::real_daemon` | private keyring (`gh:github.com` stored there for the GitHub one) | pass (real_daemon deletes its keyring now, 4b4ed710) |
| olo-system `keyring_prompt::tests::with_gcr` | `GCR_PEER` from gcr's own `Gcr.SecretExchange` (Python), its reply read back through `gcr_secret_exchange_get_secret` | pass: gcr reads "olo-secret" |
| olo-session `keyring::live::real_daemon` | private keyring; `KEYRING_TEST_DIR` its control folder | pass |
| olo-secrets `takes_over_and_hands_back` | a private keyring of its own (`KEYRING_TEST_BUS`) | pass: 300 lookups in 2.7 s |
| olo-token-pool `claude_code_on_its_subscription_through_the_proxy`, `codex_on_its_subscription_through_the_proxy`, `codex_daemon_follows_the_pool` | `CLAUDE_BIN`, `CODEX_BIN` (the installed ones); fake subscriptions, a fake provider, the network refused, HOME and their config in /var/tmp | pass: every model request signed with the pool's token |

## olo-system, this machine

| Test | How | 2026-11-08 |
|---|---|---|
| `audio::tests::live_devices`, `audio::listening_tests::listening_live`, `audio::live_write::rewrite_input_unchanged`, `audio::mic_mute_tests::mic_key`, `audio::no_output_tests::live_no_output` | PipeWire sandbox (the writing ones never on the person's server); `live_devices` and `listening_live` also read the person's, read-only | pass |
| `recording::tests::records_what_plays_and_switches_takes_in_place` | PipeWire sandbox | pass |
| `live::dconf_write` | private bus (dconf's service writes the private HOME's database); `DCONF_KEY`, `DCONF_VALUE` a test key | pass: read back there, the person's dconf without it |
| `backlight::tests::live_read`, `live_read_keyboard`, `live_connectors`, `live_rewrite` | as is (`live_rewrite` writes the level it read) | pass: 656 of 656 written back |
| `hid_monitor::tests::live_read`, `live_full` | `live_full` only when it reads full already (it sets full) | pass: 60000 of 60000 |
| `edid`, `gpus`, `gpu_load`, `gpu_tuning` (`live`, `live_nvidia_detail`), `meters`, `rfkill`, `power_profile`, `packages`, `battery::limit_tests::live_charge_limit`, `presence` (the sensor hub's, not a camera), `live::status` | as is, read-only | pass |
| `priority::tests::makes_a_thread_realtime` | as is (asks rtkit) | pass, after a0dad3dc (rtkit's realtime threads read as realtime) |
| `procs::timing::a_looks_cost` | as is | pass: 8.1 ms a look, known |
| `auth::tests::live_wrong_password` | as is: `password-auth` here is pam_unix alone (no face, reader or faillock); and against a PAM service of its own (debug build) | pass both: denied; one authentication failure in the journal for the real one |
| `fingerprint::tests::live_start_and_stop` | the real reader, with the person's OK and a finger enrolled (`FPRINT_USER`, else theirs); or fake-readers on a private bus (`OLO_FPRINT_BUS`, debug build) | pass on both: the ThinkPad's Synaptics reader (press): Ready, no match when stopped, released, claimed again |

## Not tests

Documentation examples fenced `ignore` (olo-kit-base's, Smithay's,
two of them in generated bindings) are illustrations, not meant to
compile. (The table's results are 2026-11-08's, when the first three
rows were GPUI's and the kit's was olo-ui's: the same tests, moved
with the code on 2026-11-11.)

Of the 115 tests: 111 pass as run above, 4 haven't run (the cameras;
the Mac's palm captures). The Viewer's sample tests ran on clips made as
each test's comment says, which count (Ben, 2026-11-08: the originals
were the Mac's scratch).

## Olo Coder

`crates/coder/tests/live.rs`: `pool_agent_writes_and_verifies_rust` creates a
scratch workspace under `/var/tmp`, asks the real token pool to write and test a
Rust function, and removes that workspace on success.
`pool_codex_reasoning_compaction_and_replay` and
`pool_claude_reasoning_compaction_and_replay` check encrypted reasoning and signed
compaction through each provider. `pool_automatic_review_approves_a_scoped_edit`
checks a real reviewer against explicit approval guidance. Run with a scratch `TMPDIR`:
`cargo test -p olo-coder-core --test live -- --ignored --nocapture`.
These spend subscription quota; none changes the person's source tree.
