//! Battery levels of the last 7 days, per device: the hover card's chart (last 24 hours), the
//! dashboard and the statistics.
//!
//! Only real readings are stored. A reading is stored when the level or the charging state
//! changes, and otherwise at most every 10 minutes; the latest reading in between is kept in
//! memory and written when it ends a stretch of readings. A stretch ("segment") ends when a
//! device was not read for longer than the gap limit: the app was closed, the PC was off or the
//! device was disconnected. The first point after such a gap carries the START flag, so charts
//! and statistics never bridge time that was not measured.
//!
//! Stored in `%LOCALAPPDATA%\BatteryHub\<edition>\History.bin`, written to a temporary file next
//! to it and then moved over it, so a crash never leaves half a file. Format: `BHH2`, then per
//! device `key length (u8) | key (UTF-8) | point count (u32 LE) | points (time u32 LE unix
//! seconds, level u8, flags u8)`. The 24-hour history of v1.5 (REG_BINARY `History` in the
//! settings key) is converted on the first start.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How long points are kept.
pub const KEEP_SECS: u64 = 7 * 24 * 3600;
/// An unchanged level is stored again after this long.
pub const STEP_SECS: u64 = 600;
/// The device was charging (on power) at this reading.
pub const CHARGING: u8 = 1;
/// First reading after a gap: not connected to the point before it.
pub const START: u8 = 2;
/// The device reported a full battery.
pub const FULL: u8 = 4;
const MAGIC: &[u8; 4] = b"BHH2";
const OLD_VALUE: &str = "History";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub t: u64,
    pub level: u8,
    pub flags: u8,
}

impl Point {
    pub fn charging(&self) -> bool {
        self.flags & CHARGING != 0
    }
    pub fn starts(&self) -> bool {
        self.flags & START != 0
    }
    pub fn full(&self) -> bool {
        self.flags & FULL != 0
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
struct Series {
    pts: Vec<Point>,
    /// Latest reading that was not stored (same level and state as the last stored point).
    pending: Option<Point>,
}

#[derive(Default, Debug, PartialEq, Clone)]
pub struct History {
    devices: HashMap<String, Series>,
    dirty: bool,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Longest time between two readings that still counts as continuous monitoring, for a refresh
/// interval of `minutes`.
pub fn gap_limit(minutes: u32) -> u64 {
    (2 * minutes as u64 * 60).max(15 * 60) + 60
}

impl History {
    /// Adds a reading; returns true when a point was stored. `gap` comes from `gap_limit`.
    pub fn record(&mut self, key: &str, now: u64, level: i32, charging: bool, full: bool, gap: u64) -> bool {
        if !(0..=100).contains(&level) {
            return false;
        }
        let flags = if charging { CHARGING } else { 0 } | if full { FULL } else { 0 };
        let obs = Point { t: now, level: level as u8, flags };
        let s = self.devices.entry(key.to_string()).or_default();
        let Some(&last) = s.pts.last() else {
            s.pts.push(Point { flags: flags | START, ..obs });
            self.dirty = true;
            return true;
        };
        let seen = s.pending.map(|p| p.t).unwrap_or(last.t);
        if now < seen || now - seen > gap {
            // Not read for a while: the stretch ended at the last reading, a new one starts.
            if let Some(p) = s.pending.take() {
                s.pts.push(p);
            }
            s.pts.push(Point { flags: flags | START, ..obs });
            self.dirty = true;
            return true;
        }
        let state = |p: &Point| p.flags & (CHARGING | FULL);
        if last.level != obs.level || state(&last) != state(&obs) || now - last.t >= STEP_SECS {
            s.pending = None;
            s.pts.push(obs);
            self.dirty = true;
            true
        } else {
            s.pending = Some(obs);
            false
        }
    }

    /// Stores the readings kept in memory (before saving or exiting).
    pub fn flush(&mut self) {
        for s in self.devices.values_mut() {
            if let Some(p) = s.pending.take() {
                s.pts.push(p);
                self.dirty = true;
            }
        }
    }

    /// Drops points older than 7 days and devices left without points. The newest point before
    /// the cut stays while the stretch goes on, so the level at the start of the 7 days is known.
    /// True if anything went.
    pub fn prune(&mut self, now: u64) -> bool {
        let from = now.saturating_sub(KEEP_SECS);
        let mut changed = false;
        for s in self.devices.values_mut() {
            let n = s.pts.len();
            s.pts.retain(|p| p.t <= now + 300);
            let first = s.pts.iter().position(|p| p.t >= from).unwrap_or(s.pts.len());
            let keep_from = if first < s.pts.len() && first > 0 && !s.pts[first].starts() { first - 1 } else { first };
            s.pts.drain(..keep_from);
            if s.pts.is_empty() && s.pending.is_some_and(|p| p.t < from) {
                s.pending = None;
            }
            changed |= s.pts.len() != n;
        }
        let before = self.devices.len();
        self.devices.retain(|_, s| !s.pts.is_empty() || s.pending.is_some());
        changed |= before != self.devices.len();
        self.dirty |= changed;
        changed
    }

    /// Stored points of one device, oldest first.
    pub fn points(&self, key: &str) -> &[Point] {
        self.devices.get(key).map(|s| s.pts.as_slice()).unwrap_or(&[])
    }

    /// Stored points plus the latest reading kept in memory.
    pub fn series(&self, key: &str) -> Vec<Point> {
        let Some(s) = self.devices.get(key) else { return Vec::new() };
        let mut v = s.pts.clone();
        v.extend(s.pending);
        v
    }

    /// Combines the readings of two connections of one physical device under `to` (#644).
    /// Points are sorted by time (the kept key wins an exact tie) and START flags are rebuilt
    /// with `gap`, so the shared statistics never bridge time that was not measured.
    pub fn merge_series(&mut self, from: &str, to: &str, gap: u64) -> bool {
        if from == to {
            return false;
        }
        self.flush();
        let mut moved = false;
        for suffix in ["", "#L", "#R"] {
            let src = format!("{from}{suffix}");
            let dst = format!("{to}{suffix}");
            let Some(other) = self.devices.remove(&src) else { continue };
            moved = true;
            let keep = self.devices.entry(dst).or_default();
            let mut pts: Vec<Point> = keep.pts.drain(..).chain(other.pts).collect();
            pts.sort_by_key(|p| p.t);
            let mut prev: Option<u64> = None;
            for p in &mut pts {
                p.flags &= !START;
                if prev.is_none_or(|t| p.t.saturating_sub(t) > gap) {
                    p.flags |= START;
                }
                prev = Some(p.t);
            }
            keep.pts = pts;
            if let Some(p) = other.pending
                && keep.pending.is_none_or(|q| p.t >= q.t)
            {
                keep.pending = Some(p);
            }
        }
        if moved {
            self.dirty = true;
        }
        moved
    }

    /// Latest reading of a device (stored or not).
    pub fn latest(&self, key: &str) -> Option<Point> {
        let s = self.devices.get(key)?;
        s.pending.or(s.pts.last().copied())
    }

    /// Deletes every reading of a device (and of its parts, e.g. "key#L" / "key#R" for earbuds).
    /// True if anything was there.
    pub fn remove(&mut self, key: &str) -> bool {
        let part = format!("{key}#");
        let before = self.devices.len();
        self.devices.retain(|k, _| k != key && !k.starts_with(&part));
        let changed = before != self.devices.len();
        self.dirty |= changed;
        changed
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.devices.keys()
    }

    /// Changes not saved yet.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut keys: Vec<&String> = self.devices.keys().collect();
        keys.sort();
        let mut out = MAGIC.to_vec();
        for k in keys {
            let kb = k.as_bytes();
            let pts = &self.devices[k].pts;
            if kb.len() > 255 || pts.is_empty() {
                continue;
            }
            out.push(kb.len() as u8);
            out.extend_from_slice(kb);
            out.extend_from_slice(&(pts.len() as u32).to_le_bytes());
            for p in pts {
                out.extend_from_slice(&(p.t.min(u32::MAX as u64) as u32).to_le_bytes());
                out.push(p.level);
                out.push(p.flags);
            }
        }
        out
    }

    /// Reads the format written by `encode`; stops at the first damaged entry.
    pub fn decode(b: &[u8]) -> Self {
        let mut h = History::default();
        let Some(b) = b.strip_prefix(MAGIC.as_slice()) else { return h };
        let mut i = 0;
        while i < b.len() {
            let kl = b[i] as usize;
            let Some(key) = b.get(i + 1..i + 1 + kl).and_then(|k| std::str::from_utf8(k).ok()) else { break };
            i += 1 + kl;
            let Some(n) = b.get(i..i + 4).map(|n| u32::from_le_bytes([n[0], n[1], n[2], n[3]]) as usize) else { break };
            i += 4;
            let Some(raw) = n.checked_mul(6).and_then(|len| b.get(i..i + len)) else { break };
            i += n * 6;
            let mut pts: Vec<Point> = raw
                .as_chunks::<6>()
                .0
                .iter()
                .map(|c| Point { t: u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as u64, level: c[4].min(100), flags: c[5] & (CHARGING | START | FULL) })
                .collect();
            // Out-of-order points (a clock set back) would break the charts: keep time order.
            pts.dedup_by(|b, a| b.t < a.t);
            if !pts.is_empty() {
                pts[0].flags |= START;
                h.devices.insert(key.to_string(), Series { pts, pending: None });
            }
        }
        h
    }

    /// The 24-hour REG_BINARY format of v1.5: `key length | key | count (u16) | (time u32, level u8)`.
    pub fn decode_v1(b: &[u8]) -> Self {
        let mut h = History::default();
        let mut i = 0;
        while i < b.len() {
            let kl = b[i] as usize;
            let Some(key) = b.get(i + 1..i + 1 + kl).and_then(|k| std::str::from_utf8(k).ok()) else { break };
            i += 1 + kl;
            let Some(n) = b.get(i..i + 2).map(|n| u16::from_le_bytes([n[0], n[1]]) as usize) else { break };
            i += 2;
            let Some(raw) = b.get(i..i + n * 5) else { break };
            i += n * 5;
            let mut pts: Vec<Point> = Vec::with_capacity(n);
            for c in raw.as_chunks::<5>().0 {
                let t = u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as u64;
                // v1.5 stored a point at least every 10 minutes while it read the device: a longer
                // pause means it was not read in between.
                let gap = pts.last().is_none_or(|p: &Point| t < p.t || t - p.t > gap_limit(1));
                pts.push(Point { t, level: c[4].min(100), flags: if gap { START } else { 0 } });
            }
            if !pts.is_empty() {
                h.devices.insert(key.to_string(), Series { pts, pending: None });
            }
        }
        h
    }

    /// `%LOCALAPPDATA%\BatteryHub\<edition short name>\History.bin`
    pub fn path(edition: &str) -> PathBuf {
        crate::util::data_dir(edition).join("History.bin")
    }

    /// Loads the history; on the first start after v1.5, converts the old registry value.
    pub fn load(edition: &str, settings_key: &str) -> Self {
        let path = Self::path(edition);
        if let Ok(b) = std::fs::read(&path) {
            return Self::decode(&b);
        }
        match crate::util::reg_get_binary(settings_key, OLD_VALUE) {
            Some(b) => {
                let mut h = Self::decode_v1(&b);
                h.dirty = true;
                if h.save_to(&path).is_ok() {
                    crate::util::reg_delete_value(settings_key, OLD_VALUE);
                }
                h
            }
            None => Self::default(),
        }
    }

    /// Writes the history now (used at exit); clears the dirty flag on success.
    pub fn save_to(&mut self, path: &Path) -> std::io::Result<()> {
        write_atomic(path, &self.encode())?;
        self.dirty = false;
        Ok(())
    }

    /// Hands the encoded history to a short-lived thread that writes it, so the UI thread never
    /// waits for the disk. Returns false while an earlier write is still running (try later).
    pub fn save_in_background(&mut self, path: PathBuf, done: impl FnOnce(bool) + Send + 'static) -> bool {
        use std::sync::atomic::{AtomicBool, Ordering};
        static BUSY: AtomicBool = AtomicBool::new(false);
        if BUSY.swap(true, Ordering::AcqRel) {
            return false;
        }
        let data = self.encode();
        self.dirty = false;
        std::thread::spawn(move || {
            let ok = write_atomic(&path, &data).is_ok();
            BUSY.store(false, Ordering::Release);
            done(ok);
        });
        true
    }

    /// Marks the history as changed again (a background write failed).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// Writes `data` to a temporary file in the same folder, flushes it, then moves it over `path`
/// (MoveFileEx with REPLACE_EXISTING | WRITE_THROUGH). On any error the old file is untouched and
/// the temporary file is removed.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        crate::util::replace_file(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The stretches of readings in [from, to], oldest first. A stretch that began before `from`
/// starts at `from` with the level it had then (the last reading before it), so a chart starts at
/// the left edge without inventing a value: between two readings the level is the earlier one.
pub fn segments(pts: &[Point], from: u64, to: u64) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut before: Option<Point> = None;
    for &p in pts {
        if p.t > to {
            break;
        }
        if p.t < from {
            before = Some(p);
            continue;
        }
        if p.starts() || out.is_empty() {
            let mut seg = Vec::new();
            if let (false, Some(b)) = (p.starts(), before.take()) {
                seg.push(Point { t: from, flags: b.flags | START, ..b });
            }
            seg.push(p);
            out.push(seg);
        } else if let Some(seg) = out.last_mut() {
            seg.push(p);
        }
        before = None;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: u64 = 16 * 60;

    #[test]
    fn keeps_changes_and_every_ten_minutes() {
        let mut h = History::default();
        assert!(h.record("m", 1000, 80, false, false, GAP));
        assert!(!h.record("m", 1060, 80, false, false, GAP)); // same level, 1 min later
        assert!(h.record("m", 1120, 79, false, false, GAP)); // changed
        assert!(h.record("m", 1120 + 600, 79, false, false, GAP)); // unchanged but 10 min later
        assert!(!h.record("m", 2000, -1, false, false, GAP)); // unknown level is not stored
        assert!(h.record("m", 2100, 79, true, false, GAP)); // plugged in: state change is stored
        assert_eq!(h.points("m").len(), 4);
        assert!(h.points("m")[0].starts());
        assert!(h.points("m")[3].charging());
    }

    #[test]
    fn a_gap_ends_the_stretch_at_the_last_reading() {
        let mut h = History::default();
        h.record("m", 0, 90, false, false, GAP);
        assert!(!h.record("m", 300, 90, false, false, GAP)); // kept in memory only
        // Two hours without a reading (PC off): the 300 s reading is stored, then a new stretch.
        assert!(h.record("m", 7500, 70, false, false, GAP));
        let p = h.points("m");
        assert_eq!(p.iter().map(|p| (p.t, p.starts())).collect::<Vec<_>>(), vec![(0, true), (300, false), (7500, true)]);
        // No line across the gap.
        let segs = segments(p, 0, 8000);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].last().unwrap().t, 300);
    }

    #[test]
    fn merging_connections_combines_points_and_rebuilds_gaps() {
        let mut h = History::default();
        h.record("bt", 0, 50, false, false, GAP);
        h.record("bt", 60, 49, false, false, GAP);
        h.record("2g", 30, 48, false, false, GAP);
        h.record("2g", 2000, 47, true, false, GAP); // a long pause ends the first stretch
        h.record("bt#L", 0, 70, false, false, GAP);
        h.record("2g#L", 100, 69, false, false, GAP);
        assert!(h.merge_series("bt", "2g", GAP));
        assert!(!h.merge_series("bt", "2g", GAP)); // nothing left to move
        assert!(!h.merge_series("2g", "2g", GAP));
        assert!(h.points("bt").is_empty() && h.points("bt#L").is_empty());
        let p = h.points("2g");
        assert_eq!(p.iter().map(|p| (p.t, p.level, p.starts())).collect::<Vec<_>>(), vec![
            (0, 50, true),
            (30, 48, false),
            (60, 49, false),
            (2000, 47, true),
        ]);
        assert!(p[3].charging());
        let l = h.points("2g#L");
        assert_eq!(l.iter().map(|p| (p.t, p.starts())).collect::<Vec<_>>(), vec![(0, true), (100, false)]);
    }

    #[test]
    fn flush_stores_the_latest_reading() {
        let mut h = History::default();
        h.record("m", 0, 50, false, false, GAP);
        h.record("m", 120, 50, false, false, GAP);
        assert_eq!(h.latest("m").unwrap().t, 120);
        assert_eq!(h.points("m").len(), 1);
        h.flush();
        assert_eq!(h.points("m").len(), 2);
    }

    #[test]
    fn prunes_after_7_days_but_keeps_the_level_at_the_cut() {
        let mut h = History::default();
        h.record("a", 10, 50, false, false, u64::MAX);
        h.record("a", 10 + KEEP_SECS, 40, false, false, u64::MAX);
        h.record("b", 20, 90, false, false, GAP);
        assert!(h.prune(10 + KEEP_SECS + 15));
        // a@10 continues into the kept point, so it stays as the level at the window's start.
        assert_eq!(h.points("a").len(), 2);
        assert!(h.points("b").is_empty());
        // Once a newer point is past the cut too, it goes.
        h.record("a", 20 + KEEP_SECS + 700, 39, false, false, u64::MAX);
        h.prune(20 + 2 * KEEP_SECS);
        assert_eq!(h.points("a").len(), 2);
        assert_eq!(h.points("a")[0].t, 10 + KEEP_SECS);
    }

    #[test]
    fn round_trip_and_damaged_data() {
        let mut h = History::default();
        h.record("logi:unit:ABC", 1_700_000_000, 85, false, false, GAP);
        h.record("logi:unit:ABC", 1_700_000_600, 84, true, false, GAP);
        h.record("steelseries:2269", 1_700_000_100, 99, true, true, GAP);
        h.dirty = false;
        let b = h.encode();
        assert_eq!(History::decode(&b), h);
        let cut = History::decode(&b[..b.len() - 3]);
        assert!(cut.points("logi:unit:ABC").len() == 2 && cut.points("steelseries:2269").is_empty());
        assert_eq!(History::decode(&[]), History::default());
        assert_eq!(History::decode(b"BHH2\x05ab"), History::default());
        // A huge count does not allocate or panic.
        assert_eq!(History::decode(b"BHH2\x01a\xFF\xFF\xFF\xFF"), History::default());
    }

    #[test]
    fn converts_the_v1_5_registry_format() {
        let mut b = Vec::new();
        b.push(1);
        b.push(b'm');
        b.extend_from_slice(&3u16.to_le_bytes());
        for (t, l) in [(1000u32, 80u8), (1600, 79), (9000, 60)] {
            b.extend_from_slice(&t.to_le_bytes());
            b.push(l);
        }
        let h = History::decode_v1(&b);
        let p = h.points("m");
        assert_eq!(p.len(), 3);
        assert!(p[0].starts() && !p[1].starts() && p[2].starts()); // 2 h pause = not measured
    }

    #[test]
    fn segments_start_at_the_window_edge_with_the_held_level() {
        let pts = [Point { t: 0, level: 90, flags: START }, Point { t: 500, level: 80, flags: 0 }, Point { t: 1500, level: 70, flags: 0 }];
        let s = segments(&pts, 1000, 2000);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0][0].t, s[0][0].level), (1000, 80));
        // A stretch that ended before the window is not drawn.
        let pts = [Point { t: 0, level: 90, flags: START }, Point { t: 3000, level: 50, flags: START }];
        let s = segments(&pts, 1000, 4000);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0][0].t, 3000);
    }

    #[test]
    fn atomic_write_replaces_and_keeps_old_file_on_error() {
        let dir = std::env::temp_dir().join(format!("bh-hist-test-{}", std::process::id()));
        let path = dir.join("History.bin");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        // No temporary file left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_deletes_the_device_and_its_buds() {
        let mut h = History::default();
        for k in ["pods", "pods#L", "pods#R", "podsX", "mouse"] {
            h.record(k, 1000, 50, false, false, 900);
        }
        assert!(h.remove("pods"));
        let mut left: Vec<&String> = h.keys().collect();
        left.sort();
        assert_eq!(left, ["mouse", "podsX"]);
        assert!(!h.remove("pods"));
    }
}
