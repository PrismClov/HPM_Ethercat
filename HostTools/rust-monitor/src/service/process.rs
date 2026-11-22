// SPDX-License-Identifier: GPL-3.0-only
//! Rust port of SimpleCAT's process-variable and CiA402 command handling.
//! Upstream: tc3_scan.c and ui/ui_master.c at 7bf6be13.
use super::{helper, number, Result};
use crate::backend::Backend;
use std::{
    io::{self, BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) struct Wire {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Wire {
    pub(super) fn open(command: &mut Command) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Cannot start backend: {e}"))?;
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().ok_or("Missing backend stdout.")?);
        Ok(Self {
            child,
            input,
            output,
        })
    }

    pub(super) fn send(&mut self, line: &str) -> Result<()> {
        writeln!(
            self.input.as_mut().ok_or("Closed backend stdin.")?,
            "{line}"
        )
        .map_err(|e| format!("Cannot send backend command: {e}"))
    }

    pub(super) fn response(&mut self, mut handle: impl FnMut(&[&str]) -> Result<()>) -> Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if self
                .output
                .read_line(&mut line)
                .map_err(|e| e.to_string())?
                == 0
            {
                return Err("Backend exited before completing the operation.".into());
            }
            let fields: Vec<_> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
            match fields.as_slice() {
                ["ecat", "end"] => return Ok(()),
                ["ecat", "error" | "rejected", message] => return Err((*message).to_owned()),
                ["ecat", rest @ ..] => handle(rest)?,
                _ => {}
            }
        }
    }

    pub(super) fn finish(mut self) -> Result<()> {
        self.input.take();
        io::copy(&mut self.output, &mut io::sink()).map_err(|e| e.to_string())?;
        let status = self.child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Backend cleanup failed ({status})."))
        }
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.input.take(); // EOF requests neutral output and release.
        let _ = io::copy(&mut self.output, &mut io::sink());
        let _ = self.child.wait();
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub station: u32,
    pub output: bool,
    pub pdo: u16,
    pub index: u16,
    pub sub: u8,
    pub width: u8,
    pub offset: u32,
}

struct Image {
    wire: Option<Wire>,
    simulated_since: Instant,
    entries: Vec<Entry>,
    values: Vec<Option<u64>>,
    valid: bool,
    wkc: i32,
    expected: i32,
    dc_time_ns: Option<i64>,
    al: std::sync::Arc<Vec<Al>>,
    drop_demo_wkc: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Al {
    pub station: u32,
    pub state: Option<u16>,
    pub code: Option<u16>,
}

pub struct Cycle<'a> {
    pub id: u64,
    pub time: Duration,
    pub jitter_us: Option<f64>,
    pub values: &'a [Option<u64>],
    pub valid: bool,
    pub wkc: i32,
    pub expected: i32,
    pub al: &'a std::sync::Arc<Vec<Al>>,
}

fn parse<T: std::str::FromStr>(value: &str) -> Result<T> {
    value
        .parse()
        .map_err(|_| format!("Invalid backend number {value:?}."))
}

impl Image {
    fn demo() -> Self {
        let entries = [
            (0, true, 0x7010, 32, 0),
            (0, false, 0x6000, 32, 32),
            (1, true, 0x6040, 16, 64),
            (1, false, 0x6041, 16, 80),
            (1, true, 0x6060, 8, 96),
            (1, false, 0x6061, 8, 104),
        ]
        .into_iter()
        .map(|(station, output, index, width, offset)| Entry {
            station,
            output,
            pdo: if output { 0x1600 } else { 0x1a00 },
            index,
            sub: 0,
            width,
            offset,
        })
        .collect();
        Self {
            wire: None,
            simulated_since: Instant::now(),
            entries,
            values: vec![Some(0), Some(0), Some(0), Some(0x40), Some(8), Some(8)],
            valid: true,
            wkc: 6,
            expected: 6,
            dc_time_ns: None,
            al: std::sync::Arc::new(
                (0..2)
                    .map(|station| Al {
                        station,
                        state: Some(8),
                        code: Some(0),
                    })
                    .collect(),
            ),
            drop_demo_wkc: false,
        }
    }

    fn simulate(&mut self, command: &str) -> Result<()> {
        self.valid = !std::mem::take(&mut self.drop_demo_wkc);
        self.wkc = if self.valid { 6 } else { 0 };
        let fields: Vec<_> = command.split_whitespace().collect();
        if let ["W", id, value] = fields.as_slice() {
            let id = parse::<usize>(id)?;
            let value = parse::<u64>(value)?;
            self.write_command(id, value)?;
            self.values[id] = Some(value);
            if id == 2 {
                self.values[3] = Some(match (value, self.values[3]) {
                    (0x80, Some(8)) => 0x40,
                    (6, Some(0x40)) => 0x21,
                    (7, Some(0x21 | 0x27)) => 0x23,
                    (15, Some(0x23)) => 0x27,
                    (2, _) => 7,
                    (0, Some(8)) => 8,
                    (0, _) => 0x40,
                    (_, status) => status.unwrap_or(0),
                });
            } else if id == 4 {
                self.values[5] = Some(value);
            }
        }
        let ticks = self.simulated_since.elapsed().as_millis() as u64 / 200;
        self.values[1] =
            Some(((ticks % 20) << 16) | ((ticks ^ self.values[0].unwrap_or(0)) & 0xffff));
        Ok(())
    }

    fn open(mut wire: Wire) -> Result<Self> {
        let mut entries = Vec::new();
        wire.response(|row| {
            let ["entry", id, station, direction, pdo, index, sub, width, offset, _name] = row
            else {
                return Err("Unexpected PDO layout record.".into());
            };
            if parse::<usize>(id)? != entries.len()
                || entries.len() >= 16384
                || !matches!(*direction, "0" | "1")
            {
                return Err("Invalid backend layout order or direction.".into());
            }
            entries.push(Entry {
                station: parse(station)?,
                output: *direction == "1",
                pdo: parse(pdo)?,
                index: parse(index)?,
                sub: parse(sub)?,
                width: parse(width)?,
                offset: parse(offset)?,
            });
            Ok(())
        })?;
        if entries.is_empty() {
            return Err("Bus has no mapped PDO variables.".into());
        }
        let values = vec![None; entries.len()];
        Ok(Self {
            wire: Some(wire),
            simulated_since: Instant::now(),
            entries,
            values,
            valid: false,
            wkc: 0,
            expected: -1,
            dc_time_ns: None,
            al: Default::default(),
            drop_demo_wkc: false,
        })
    }

    fn cycle(&mut self, command: &str) -> Result<()> {
        let Some(wire) = &mut self.wire else {
            return self.simulate(command);
        };
        wire.send(command)?;
        self.valid = false;
        self.values.fill(None);
        self.dc_time_ns = None;
        let mut al = Vec::new();
        wire.response(|row| {
            match row {
                ["cycle", valid, wkc, expected] => {
                    self.valid = *valid == "1";
                    self.wkc = parse(wkc)?;
                    self.expected = parse(expected)?;
                }
                ["value", id, value] if self.valid => {
                    let slot = self
                        .values
                        .get_mut(parse::<usize>(id)?)
                        .ok_or("Invalid variable ID.")?;
                    *slot = Some(parse(value)?);
                }
                ["dc", ns] => self.dc_time_ns = Some(parse(ns)?),
                ["al", station, state, code] => al.push(Al {
                    station: parse(station)?,
                    state: u16::try_from(parse::<i32>(state)?).ok(),
                    code: u16::try_from(parse::<i32>(code)?).ok(),
                }),
                _ => return Err("Unexpected PDO cycle record.".into()),
            }
            Ok(())
        })?;
        if !al.is_empty() && *self.al != al {
            self.al = std::sync::Arc::new(al);
        }
        Ok(())
    }

    fn lookup(&self, station: u32, index: u16, output: bool, width: u8) -> Result<usize> {
        let mut matching = self.entries.iter().enumerate().filter(|(_, e)| {
            (e.station, e.index, e.sub, e.output, e.width) == (station, index, 0, output, width)
        });
        match (matching.next(), matching.next()) {
            (Some((id, _)), None) => Ok(id),
            _ => Err(format!(
                "Expected one mapped {index:04x}:00 ({width} bits) on station {station}."
            )),
        }
    }

    fn write_command(&self, id: usize, value: u64) -> Result<String> {
        let e = self.entries.get(id).ok_or("Unknown variable ID.")?;
        if !self.valid
            || !e.output
            || e.width == 0
            || e.width > 64
            || (e.width < 64 && value >= 1 << e.width)
        {
            return Err(
                "Write requires valid OP data, a writable output and an in-range value.".into(),
            );
        }
        Ok(format!("W {id} {value}"))
    }

    fn finish(self) -> Result<()> {
        self.wire.map_or(Ok(()), Wire::finish)
    }
}

fn print_layout(entries: &[Entry]) {
    println!("ID\tStation\tDirection\tPDO\tObject\tBits\tIOmap bit");
    for (id, e) in entries.iter().enumerate() {
        println!(
            "{id}\t{}\t{}\t{:04X}\t{:04X}:{:02X}\t{}\t{}",
            e.station,
            if e.output { "Rx" } else { "Tx" },
            e.pdo,
            e.index,
            e.sub,
            e.width,
            e.offset
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveState {
    NotReady,
    Disabled,
    Ready,
    SwitchedOn,
    Enabled,
    QuickStop,
    Reaction,
    Fault,
    Unknown,
}

pub fn drive_state(word: u16) -> DriveState {
    match (word & 0x004f, word & 0x006f) {
        (0x0000, _) => DriveState::NotReady,
        (0x0040, _) => DriveState::Disabled,
        (_, 0x0021) => DriveState::Ready,
        (_, 0x0023) => DriveState::SwitchedOn,
        (_, 0x0027) => DriveState::Enabled,
        (_, 0x0007) => DriveState::QuickStop,
        (0x000f, _) => DriveState::Reaction,
        (0x0008, _) => DriveState::Fault,
        _ => DriveState::Unknown,
    }
}

fn enable_step(state: DriveState) -> Result<Option<u64>> {
    match state {
        DriveState::Disabled => Ok(Some(0x06)),
        DriveState::Ready => Ok(Some(0x07)),
        DriveState::SwitchedOn => Ok(Some(0x0f)),
        DriveState::Enabled => Ok(None),
        _ => Err(format!("Cannot enable from {state:?}.")),
    }
}

enum DriveAction {
    Enable {
        written: Option<u64>,
    },
    Reset {
        release_at: Option<Instant>,
        released: bool,
    },
}

struct DriveJob {
    control: usize,
    status: usize,
    deadline: Instant,
    action: DriveAction,
}

impl DriveJob {
    fn next(&mut self, image: &Image, now: Instant) -> Result<(bool, Option<String>)> {
        let state = image.values[self.status]
            .map(|v| drive_state(v as u16))
            .ok_or("CiA402 interrupted by invalid process data.")?;
        if now >= self.deadline {
            return Err("CiA402 transition timed out.".into());
        }
        let command = match &mut self.action {
            DriveAction::Enable { written } => {
                let Some(value) = enable_step(state)? else {
                    return Ok((true, None));
                };
                if written.is_some_and(|previous| value <= previous) {
                    return Ok((false, None));
                }
                *written = Some(value);
                value
            }
            DriveAction::Reset { released, .. } if *released => {
                return Ok((
                    !matches!(state, DriveState::Fault | DriveState::Reaction),
                    None,
                ));
            }
            DriveAction::Reset {
                release_at,
                released,
            } => match *release_at {
                Some(due) if now < due => return Ok((false, None)),
                Some(_) => {
                    *released = true;
                    0
                }
                None if state == DriveState::Fault => {
                    *release_at = Some(now + Duration::from_millis(100));
                    0x80
                }
                None => return Err("Fault reset requires a drive in Fault.".into()),
            },
        };
        Ok((false, Some(image.write_command(self.control, command)?)))
    }
}

#[derive(Clone)]
pub struct Options {
    pub backend: Backend,
    pub target: String,
    pub period_us: u32,
    pub assign_activate: u16,
    pub writable: bool,
}

pub enum Event {
    Layout(Vec<Entry>),
    Sample {
        values: Vec<Option<u64>>,
        valid: bool,
        wkc: i32,
        expected: i32,
    },
    Notice(String),
    Rejected(String),
}

pub(super) fn run(backend: Backend, target: &str, us: u32, dc: u16, writable: bool) -> Result<()> {
    println!("Commands: list | read ID | set ID VALUE | drive STATION status|enable|reset|disable|stop|mode CODE | quit");
    let (tx, rx) = mpsc::sync_channel(16);
    thread::spawn(move || {
        for line in io::stdin()
            .lock()
            .lines()
            .map_while(std::result::Result::ok)
        {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    serve(
        Options {
            backend,
            target: target.into(),
            period_us: us,
            assign_activate: dc,
            writable,
        },
        rx,
        || false,
        |event| match event {
            Event::Layout(entries) => print_layout(&entries),
            Event::Notice(text) => println!("{text}"),
            Event::Rejected(text) => eprintln!("{text}"),
            Event::Sample { .. } => {}
        },
        |_| {},
    )
}

fn current_slot_start(due: Instant, now: Instant, period: Duration) -> Instant {
    let late = now.saturating_duration_since(due);
    if late < period {
        return due;
    }
    // Skip fully elapsed slots and retain the slot that can still send.
    let elapsed = late.as_nanos() % period.as_nanos();
    now - Duration::from_nanos(elapsed as u64)
}

pub fn serve(
    options: Options,
    rx: mpsc::Receiver<String>,
    stop: impl Fn() -> bool,
    mut notify: impl FnMut(Event),
    mut record: impl FnMut(Cycle<'_>),
) -> Result<()> {
    let Options {
        backend,
        target,
        period_us: us,
        assign_activate,
        writable,
    } = options;
    if !(100..=20_000).contains(&us) {
        return Err("Period must be 100–20000 µs.".into());
    }
    let ns = (us * 1000).to_string();
    let dc = assign_activate.to_string();
    let mut image = if backend == Backend::Demo {
        Image::demo()
    } else {
        Image::open(Wire::open(helper(backend)?.args([
            &target,
            "--process",
            &ns,
            &dc,
        ]))?)?
    };
    notify(Event::Layout(image.entries.clone()));
    let period = Duration::from_micros(u64::from(us));
    let mut due = Instant::now();
    let mut scheduled_period = period;
    let mut status_at = due;
    let mut job = None::<DriveJob>;
    let mut dc_integral = 0;
    let mut ready = false;
    let mut publish_at = due;
    let started = due;
    let mut previous = None;
    let mut cycle_id = 0;
    loop {
        while Instant::now() < due {
            let remaining = due.saturating_duration_since(Instant::now());
            if remaining > Duration::from_micros(250) {
                thread::sleep(remaining - Duration::from_micros(150));
            } else {
                std::hint::spin_loop();
            }
        }
        if stop() {
            return image.finish();
        }
        let now = Instant::now();
        due = current_slot_start(due, now, scheduled_period);
        let request = match rx.try_recv() {
            Ok(line) if line.trim() == "quit" => return image.finish(),
            Ok(line) => Some(line),
            Err(mpsc::TryRecvError::Disconnected) => return image.finish(),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        let mut write = None;
        if let Some(line) = request {
            match execute(&mut image, &line, &mut job, writable, now, &mut notify) {
                Ok(command) => write = command,
                Err(error) => notify(Event::Rejected(error)),
            }
        }
        if let Some(active) = &mut job {
            match active.next(&image, now) {
                Ok((done, command)) => {
                    write = command;
                    if done {
                        job = None;
                        notify(Event::Notice("CiA402 序列完成。".into()));
                    }
                }
                // End this output session rather than retaining a partially advanced control word.
                Err(error) => return Err(error),
            }
        }
        let check = now >= status_at;
        if check && write.is_none() {
            status_at = now + Duration::from_millis(250);
        }
        image.cycle(write.as_deref().unwrap_or(if check { "S" } else { "P" }))?;
        let captured = Instant::now();
        cycle_id += 1;
        let jitter_us = previous.replace(captured).map(|previous| {
            captured.duration_since(previous).as_secs_f64() * 1_000_000.0 - f64::from(us)
        });
        record(Cycle {
            id: cycle_id,
            time: captured.duration_since(started),
            jitter_us,
            values: if image.valid { &image.values } else { &[] },
            valid: image.valid,
            wkc: image.wkc,
            expected: image.expected,
            al: &image.al,
        });
        if write.is_some() && !image.valid {
            return Err("Output cycle did not receive valid PDO acknowledgement.".into());
        }
        if write.is_some() {
            notify(Event::Notice("输出已发送，PDO 周期有效。".into()));
        }
        if image.valid && !ready {
            notify(Event::Notice(format!("PDO 已进入 OP，WKC {}。", image.wkc)));
        }
        if now >= publish_at || write.is_some() || ready != image.valid {
            publish_at = now + Duration::from_millis(40);
            notify(Event::Sample {
                values: if image.valid {
                    image.values.clone()
                } else {
                    vec![None; image.entries.len()]
                },
                valid: image.valid,
                wkc: image.wkc,
                expected: image.expected,
            });
        }
        ready = image.valid;
        let adjusted_period = if let Some(reference) = image.dc_time_ns {
            let ns = i64::from(us) * 1000;
            // Schedule SOEM sends relative to the distributed reference clock.
            // IgH disciplines its reference to application time in the helper.
            let delta = (reference.rem_euclid(ns) - 50_000 + ns / 2).rem_euclid(ns) - ns / 2;
            dc_integral = (dc_integral + delta.signum()).clamp(-ns, ns);
            let correction = (-delta / 100 - dc_integral / 20).clamp(-ns / 20, ns / 20);
            Duration::from_nanos((ns + correction) as u64)
        } else {
            dc_integral = 0;
            period
        };
        due += adjusted_period;
        scheduled_period = adjusted_period;
    }
}

fn execute(
    image: &mut Image,
    line: &str,
    job: &mut Option<DriveJob>,
    writable: bool,
    now: Instant,
    notify: &mut impl FnMut(Event),
) -> Result<Option<String>> {
    let fields: Vec<_> = line.split_whitespace().collect();
    let id = |value: &str| {
        number(value)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or("Invalid ID.")
    };
    match fields.as_slice() {
        [] => {}
        ["list"] => notify(Event::Layout(image.entries.clone())),
        ["demo", "fault"] if image.wire.is_none() => {
            job.take();
            image.values[3] = Some(8);
        }
        ["demo", "wkc"] if image.wire.is_none() => image.drop_demo_wkc = true,
        ["read", key] => {
            let value = image.values.get(id(key)?).ok_or("Unknown variable ID.")?;
            notify(Event::Notice(format!(
                "{key} = {value:?}   WKC {}/{}",
                image.wkc, image.expected
            )));
        }
        ["set", key, value] if writable && job.is_none() => {
            return image
                .write_command(id(key)?, number(value).ok_or("Invalid unsigned value.")?)
                .map(Some);
        }
        ["drive", station, action, args @ ..] => {
            let station = number(station)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or("Invalid station.")?;
            let status = image.lookup(station, 0x6041, false, 16)?;
            let state = image.values[status]
                .map(|v| drive_state(v as u16))
                .ok_or("No valid drive status.")?;
            if *action == "status" && args.is_empty() {
                let mode = image
                    .lookup(station, 0x6061, false, 8)
                    .ok()
                    .and_then(|id| image.values[id])
                    .map(|v| v as i8);
                notify(Event::Notice(format!("{state:?}   mode {mode:?}")));
                return Ok(None);
            }
            if !writable {
                return Err("Drive actions require --allow-outputs.".into());
            }
            let control = image.lookup(station, 0x6040, true, 16)?;
            if let Some(active) = job {
                if control != active.control || !matches!((*action, args), ("disable" | "stop", []))
                {
                    return Err("A CiA402 sequence is active.".into());
                }
                *job = None;
            }
            match (*action, args) {
                ("enable", []) | ("reset", []) => {
                    *job = Some(DriveJob {
                        control,
                        status,
                        deadline: now + Duration::from_secs(3),
                        action: if *action == "enable" {
                            DriveAction::Enable { written: None }
                        } else {
                            DriveAction::Reset {
                                release_at: None,
                                released: false,
                            }
                        },
                    });
                }
                ("disable", []) => return image.write_command(control, 0).map(Some),
                ("stop", []) => return image.write_command(control, 2).map(Some),
                ("mode", [mode]) if state != DriveState::Enabled => {
                    let mode: i8 = mode.parse().map_err(|_| "Invalid signed mode code.")?;
                    return image
                        .write_command(
                            image.lookup(station, 0x6060, true, 8)?,
                            u64::from(mode as u8),
                        )
                        .map(Some);
                }
                _ => return Err("Unknown drive action or mode change while enabled.".into()),
            }
        }
        _ => return Err("Unknown command or output writes are disabled.".into()),
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakeup_inside_a_slot_preserves_its_remaining_send_budget() {
        let due = Instant::now();
        let period = Duration::from_micros(1000);
        for (late, expected) in [(0, 0), (1, 0), (999, 0), (1000, 1000), (2501, 2000)] {
            let wakeup = due + Duration::from_micros(late);
            assert_eq!(
                current_slot_start(due, wakeup, period),
                due + Duration::from_micros(expected)
            );
        }
    }

    #[test]
    fn simulated_drive_follows_the_shared_cia402_sequence() {
        let mut image = Image::demo();
        let now = Instant::now();
        let mut job = DriveJob {
            control: image.lookup(1, 0x6040, true, 16).unwrap(),
            status: image.lookup(1, 0x6041, false, 16).unwrap(),
            deadline: now + Duration::from_secs(3),
            action: DriveAction::Enable { written: None },
        };
        for control in [6, 7, 15] {
            let (done, command) = job.next(&image, now).unwrap();
            assert!(!done);
            let command = command.unwrap();
            assert_eq!(command, format!("W 2 {control}"));
            image.cycle(&command).unwrap();
        }
        assert_eq!(job.next(&image, now).unwrap(), (true, None));
        image.cycle("W 2 2").unwrap();
        assert_eq!(
            drive_state(image.values[3].unwrap() as u16),
            DriveState::QuickStop
        );
    }

    #[test]
    fn enable_is_driven_by_statusword_and_rejects_fault_or_quick_stop() {
        for (status, expected) in [(0x0040, 6), (0x0021, 7), (0x0023, 15)] {
            assert_eq!(
                enable_step(drive_state(status | 0x1200)),
                Ok(Some(expected))
            );
        }
        assert_eq!(enable_step(drive_state(0x0027)), Ok(None));
        assert!(enable_step(drive_state(0x0008)).is_err());
        assert!(enable_step(drive_state(0x0007)).is_err());
    }
}
