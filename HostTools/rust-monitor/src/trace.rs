use ethercat_monitor::service::process::{drive_state, Al, Cycle, DriveState, Entry};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Pdo,
    Gpio,
    Pdi,
    Wkc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Display {
    Hex,
    Decimal,
    Bits,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Key {
    pub station: u32,
    pub pdo: u16,
    pub index: u16,
    pub sub: u8,
    pub offset: u32,
    pub width: u8,
    pub output: bool,
    pub source: Source,
}

impl Key {
    pub fn entry(entry: &Entry, source: Source) -> Self {
        Self {
            station: entry.station,
            pdo: entry.pdo,
            index: entry.index,
            sub: entry.sub,
            offset: entry.offset,
            width: entry.width,
            output: entry.output,
            source,
        }
    }

    pub fn wkc() -> Self {
        Self {
            station: 0,
            pdo: 0,
            index: 0,
            sub: 0,
            offset: 0,
            width: 32,
            output: false,
            source: Source::Wkc,
        }
    }

    pub fn bind(&self, entries: &[Entry]) -> Option<usize> {
        entries
            .iter()
            .position(|e| Key::entry(e, self.source) == *self)
    }

    pub fn caption(&self) -> String {
        match self.source {
            Source::Wkc => "WKC".into(),
            Source::Gpio => format!("{:02} GPIO", self.station),
            Source::Pdi => format!("{:02} PDI 相位 (µs)", self.station),
            Source::Pdo => format!("{:02}  {:04X}:{:02X}", self.station, self.index, self.sub),
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Watch {
    pub key: Key,
    pub display: Display,
    pub group: String,
    pub plotted: bool,
    pub color: usize,
    #[serde(skip)]
    pub id: Option<usize>,
}

impl Watch {
    pub fn value(&self, frame: &Recorded) -> Option<u64> {
        if self.key.source == Source::Wkc {
            return u64::try_from(frame.wkc).ok();
        }
        if !frame.valid {
            return None;
        }
        let value = frame.values.get(self.id?).copied().flatten()?;
        match self.key.source {
            Source::Gpio => Some(value & 0xffff),
            Source::Pdi => {
                let phase = value >> 16;
                (phase != 0xffff).then_some(phase)
            }
            _ => Some(value),
        }
    }

    pub fn formatted(&self, frame: &Recorded) -> Option<String> {
        let value = self.value(frame)?;
        Some(match self.display {
            Display::Hex => {
                let width = match self.key.source {
                    Source::Wkc => 1,
                    Source::Gpio | Source::Pdi => 4,
                    Source::Pdo => self.key.width.min(64).div_ceil(4),
                };
                format!("0x{value:0width$X}", width = usize::from(width))
            }
            Display::Decimal => value.to_string(),
            Display::Bits => {
                let width = if matches!(self.key.source, Source::Gpio | Source::Pdi) {
                    16
                } else {
                    self.key.width.min(64)
                };
                format!("{value:0width$b}", width = usize::from(width))
            }
        })
    }
}

#[derive(Clone)]
pub struct Recorded {
    pub id: u64,
    pub time: Duration,
    pub jitter_us: Option<f64>,
    pub values: Box<[Option<u64>]>,
    pub valid: bool,
    pub wkc: i32,
    pub expected: i32,
    pub al: Arc<Vec<Al>>,
}

impl From<Cycle<'_>> for Recorded {
    fn from(cycle: Cycle<'_>) -> Self {
        Self {
            id: cycle.id,
            time: cycle.time,
            jitter_us: cycle.jitter_us,
            values: cycle.values.into(),
            valid: cycle.valid,
            wkc: cycle.wkc,
            expected: cycle.expected,
            al: cycle.al.clone(),
        }
    }
}

pub struct Marker {
    pub id: u64,
    pub time: Duration,
    pub text: String,
    pub alarm: bool,
}

#[derive(Default)]
pub struct History {
    pub frames: VecDeque<Recorded>,
    pub markers: VecDeque<Marker>,
    pub cursor: Option<Recorded>,
    faults: BTreeMap<u32, bool>,
    status_entries: Vec<(u32, usize)>,
    seen_valid: bool,
    capacity: usize,
}

impl History {
    pub fn layout(&mut self, entries: &[Entry]) {
        self.capacity = (32 * 1024 * 1024
            / (std::mem::size_of::<Recorded>()
                + entries.len() * std::mem::size_of::<Option<u64>>()))
        .clamp(2, 100_000);
        self.status_entries = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.output && e.index == 0x6041 && e.sub == 0 && e.width == 16)
            .map(|(id, e)| (e.station, id))
            .collect();
    }

    pub fn push(&mut self, frame: Recorded) {
        let mut events = Vec::new();
        if let Some(previous) = self.frames.back() {
            if frame.id != previous.id + 1 {
                events.push(("记录队列出现间隔".into(), true));
            }
            if self.seen_valid
                && !frame.valid
                && (previous.valid || (previous.wkc != 0 && frame.wkc == 0))
            {
                let text = if frame.wkc == 0 {
                    if frame.expected >= 0 {
                        format!("WKC 归零（期望 {}）", frame.expected)
                    } else {
                        "WKC 归零".into()
                    }
                } else {
                    format!("PDO 无效 · WKC {}", frame.wkc)
                };
                events.push((text, true));
            }
            if !Arc::ptr_eq(&previous.al, &frame.al) {
                for al in frame.al.iter() {
                    let before = previous.al.iter().find(|old| old.station == al.station);
                    if before.is_some_and(|old| old != al) {
                        events.push((
                            format!(
                                "从站 {:02} · AL {}{}",
                                al.station,
                                al.state
                                    .map_or("无响应".into(), |value| format!("0x{value:02X}")),
                                al.code
                                    .filter(|&code| code != 0)
                                    .map_or(String::new(), |code| format!(" / 0x{code:04X}"))
                            ),
                            al.state != Some(8) || al.code.is_some_and(|code| code != 0),
                        ));
                    }
                }
            }
        }
        if frame.valid {
            for &(station, id) in &self.status_entries {
                if let Some(word) = frame.values.get(id).copied().flatten() {
                    let fault = matches!(
                        drive_state(word as u16),
                        DriveState::Fault | DriveState::Reaction
                    );
                    if self.faults.insert(station, fault) != Some(fault) && fault {
                        events.push((format!("从站 {station:02} · CiA402 故障"), true));
                    }
                }
            }
        }
        for (text, alarm) in events {
            self.markers.push_back(Marker {
                id: frame.id,
                time: frame.time,
                text,
                alarm,
            });
        }
        self.seen_valid |= frame.valid;
        let cutoff = frame.time.saturating_sub(Duration::from_secs(10));
        self.frames.push_back(frame);
        while self.frames.len() > self.capacity.max(2)
            || self.frames.front().is_some_and(|old| old.time < cutoff)
        {
            self.frames.pop_front();
        }
        let first = self.frames.front().unwrap().id;
        while self.markers.front().is_some_and(|marker| marker.id < first)
            || self.markers.len() > 2048
        {
            self.markers.pop_front();
        }
    }

    pub fn select(&mut self, id: u64) {
        let index = self.frames.partition_point(|frame| frame.id < id);
        if let Some(frame) = self.frames.get(index).filter(|frame| frame.id == id) {
            self.cursor = Some(frame.clone());
        }
    }

    pub fn observed(&self) -> Option<&Recorded> {
        self.cursor.as_ref().or_else(|| self.frames.back())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wkc_marker_selects_exact_invalid_cycle_and_snapshot_stays_frozen() {
        let mut history = History::default();
        let al = Arc::new(Vec::new());
        let sample = |id, valid| Recorded {
            id,
            time: Duration::from_millis(id),
            valid,
            values: if valid {
                Box::new([Some(id)])
            } else {
                Box::new([])
            },
            wkc: if valid { 3 } else { 0 },
            expected: 3,
            al: al.clone(),
            jitter_us: None,
        };
        history.push(sample(1, true));
        history.push(sample(2, false));
        history.push(sample(3, true));
        assert_eq!(history.markers[0].id, 2);
        history.select(history.markers[0].id);
        history.push(sample(4, true));
        let frame = history.observed().unwrap();
        assert_eq!(frame.id, 2);
        assert!(!frame.valid);
        assert!(frame.values.is_empty());
        history.cursor = None;
        assert_eq!(history.observed().unwrap().id, 4);
    }
}
