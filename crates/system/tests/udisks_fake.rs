//! `olo_system::disks` against `udisks-fake`, end to end. Needs a bus
//! to meet on: run with `OLO_TEST_BUS=unix:path=…` (a private
//! dbus-broker or busd), e.g.
//! `OLO_TEST_BUS=unix:path=/tmp/olo-dev/notes/bus.sock cargo test -p olo-system --test udisks_fake`.
//! Without it the test does nothing.

use std::{
    process::{Child, Command},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use olo_system::disks::{self, Format, Status};

struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("udisks-fake-{}", self.0.id())));
    }
}

fn wait_for(what: &str, mut ok: impl FnMut(&Status) -> bool) -> Status {
    let start = Instant::now();
    loop {
        if let Ok(s) = disks::status()
            && ok(&s)
        {
            return s;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn against_the_fake() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    // SAFETY: set before any other thread reads the environment.
    unsafe { std::env::set_var("OLO_UDISKS_BUS", &address) };
    let _fake = Fake(Command::new(env!("CARGO_BIN_EXE_udisks-fake")).args(["--address", &address]).spawn().expect("udisks-fake"));
    // A call to the name before the fake has it may wait for good.
    let bus = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().build().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus).unwrap();
    let start = Instant::now();
    while !dbus.name_has_owner("org.freedesktop.UDisks2".try_into().unwrap()).unwrap_or(false) {
        assert!(start.elapsed() < Duration::from_secs(10), "udisks-fake didn't start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = wait_for("the disks", |s| s.disks.len() == 4);

    // What it reads.
    let names: Vec<String> = s.disks.iter().map(|d| d.name()).collect();
    assert_eq!(names, ["Olo Test SSD", "SD Card", "Olo USB Stick", "Olo Backup Disk"]);
    let ssd = &s.disks[0];
    assert!(ssd.system && ssd.table.as_deref() == Some("gpt"));
    assert_eq!(ssd.volumes.iter().map(|v| v.device.as_str()).collect::<Vec<_>>(), ["/dev/nvme0n1p1", "/dev/nvme0n1p2", "/dev/nvme0n1p3"]);
    assert_eq!(ssd.volumes[2].mount_points, ["/"]);
    assert!(matches!(ssd.segments().last(), Some(disks::Segment::Free { .. })));
    let health = ssd.drive.as_ref().and_then(|d| d.health.clone()).expect("SMART");
    assert!(health.ok() && (health.temperature.unwrap() - 38.0).abs() < 0.01 && health.power_on_hours == Some(1234));
    assert!(s.filesystems.iter().any(|f| f == "exfat"));
    assert_eq!(disks::formats(&s.filesystems)[0].0, "exfat");

    // Mount and unmount the stick; the system can't be unmounted.
    let stick = s.disks.iter().find(|d| d.name() == "Olo USB Stick").unwrap().volumes[0].path.clone();
    let at = disks::mount(&stick).unwrap();
    assert!(at.ends_with("/STICK"));
    wait_for("the mount", |s| s.volume(&stick).is_some_and(|(_, v)| v.mounted()));
    disks::unmount(&stick).unwrap();
    assert!(disks::unmount(&ssd.volumes[2].path).is_err());

    // Unlock the backup disk (a wrong passphrase first), mount what's in it.
    let luks = s.disks.iter().find(|d| d.name() == "Olo Backup Disk").unwrap().volumes[0].path.clone();
    assert!(disks::unlock(&luks, "wrong").is_err());
    let clear = disks::unlock(&luks, "secret").unwrap();
    let s = wait_for("the unlock", |s| s.volume(&luks).is_some_and(|(_, v)| v.cleartext.is_some()));
    assert_eq!(s.volume(&luks).unwrap().1.title(), "Backups");
    disks::mount(&clear).unwrap();
    assert!(disks::lock(&luks).is_err(), "locking with it mounted");
    disks::unmount(&clear).unwrap();
    disks::lock(&luks).unwrap();

    // Format the stick, watching the job.
    let watcher = std::thread::spawn(|| wait_for("the job", |s| s.jobs.iter().any(|j| j.title() == "Formatting")));
    disks::format(&stick, &Format { kind: "ext4".into(), label: "Linux".into(), ..Default::default() }).unwrap();
    assert!(watcher.join().is_ok());
    let s = wait_for("the format", |s| s.volume(&stick).is_some_and(|(_, v)| v.content == "ext4"));
    assert_eq!(s.volume(&stick).unwrap().1.label, "Linux");

    // A partition table on the empty card, then a partition in its free space.
    let card = s.disks.iter().find(|d| d.name() == "SD Card").unwrap().path.clone();
    disks::new_table(&card, "gpt", false).unwrap();
    let s = wait_for("the table", |s| s.disk(&card).is_some_and(|d| d.table.is_some()));
    let Some(disks::Segment::Free { offset, size }) = s.disk(&card).unwrap().segments().first().copied() else { panic!("no free space") };
    let part = disks::create_partition(&card, "gpt", offset, size / 2, "Photos", &Format { kind: "exfat".into(), label: "PHOTOS".into(), ..Default::default() }).unwrap();
    let s = wait_for("the partition", |s| s.volume(&part).is_some_and(|(_, v)| v.content == "exfat"));
    let (_, v) = s.volume(&part).unwrap();
    assert_eq!((v.name.as_str(), v.part_type.as_str()), ("Photos", "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"));
    disks::resize(&part, v.size, v.size + (1 << 30), true).unwrap();
    disks::set_label(&part, "Pictures").unwrap();
    wait_for("the label", |s| s.volume(&part).is_some_and(|(_, v)| v.label == "Pictures"));
    disks::delete_partition(&part).unwrap();
    wait_for("the delete", |s| s.disk(&card).is_some_and(|d| d.volumes.is_empty()));

    // Write an image to the card and read it back with the benchmark.
    let image = std::env::temp_dir().join(format!("udisks-fake-test-{}.img", std::process::id()));
    std::fs::write(&image, vec![7u8; 3 << 20]).unwrap();
    let mut last = disks::Progress::default();
    disks::write_image(&card, &image, &AtomicBool::new(false), &mut |p| last = p).unwrap();
    let _ = std::fs::remove_file(&image);
    assert_eq!(last.done, 3 << 20);
    let b = disks::benchmark(&card, 64 << 30, 3, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(b.samples.len(), 3);
    assert!(b.average > 0.0);

    // Eject the stick: it goes.
    let s = disks::status().unwrap();
    let stick_disk = s.disks.iter().find(|d| d.name() == "Olo USB Stick").unwrap().clone();
    disks::eject(&stick_disk).unwrap();
    wait_for("the eject", |s| s.disks.len() == 3);
}
