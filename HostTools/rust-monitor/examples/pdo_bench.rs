use ethercat_monitor::backend::{Backend, Session, Snapshot, Update};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, process, thread,
    time::{Duration, Instant},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [kind, target, seconds, periods @ ..] = args.as_slice() else {
        return Err(
            "Usage: pdo_bench <igh|soem> <master-or-interface> <seconds-per-period> <period-us>..."
                .into(),
        );
    };
    let kind = match kind.as_str() {
        "igh" if cfg!(target_os = "linux") => Backend::Igh,
        "soem" => Backend::Soem,
        _ => return Err("Unsupported benchmark backend".into()),
    };
    let seconds: u64 = seconds.parse().map_err(|_| "Invalid duration")?;
    if !(1..=600).contains(&seconds) || periods.is_empty() {
        return Err("Enter a duration of 1–600 seconds and at least one period".into());
    }
    let periods: Vec<u64> = periods
        .iter()
        .map(|period| {
            period
                .parse::<u64>()
                .ok()
                .filter(|n| (100..=20_000).contains(n))
                .ok_or("Period must be 100–20000 µs")
        })
        .collect::<Result<_, _>>()?;
    println!("target_us,duration_s,slots,samples,valid,invalid,op_samples,bad_al,valid_hz,invalid_pct,missed_slots_pct,p50_us,p99_us,max_us,wkc_histogram,gpio_words,phase_min_us,phase_max_us,phase_invalid");
    for period in periods {
        let row = measure(kind, target, seconds, period)?;
        println!("{row}");
    }
    Ok(())
}

fn measure(kind: Backend, target: &str, seconds: u64, period: u64) -> Result<String, String> {
    let session = Session::start(
        kind,
        target.to_owned(),
        Duration::from_micros(period),
        true,
        || {},
    );
    let warmup = Instant::now() + Duration::from_secs(25);
    let mut start = None;
    let wall_time = Duration::from_secs(seconds);
    let mut counts = Counts::default();
    loop {
        for update in session.take_updates() {
            let (due, snapshot) = match update {
                Update::Snapshot { due, data } => (due, data),
                Update::Error(error) => return Err(error),
                Update::Stopped => {
                    return Err("Backend stopped before the measurement finished".into())
                }
            };
            if start.is_none()
                && is_op(&snapshot)
                && snapshot.pdo.is_some_and(|sample| sample.raw.is_some())
            {
                start = Some(due);
            }
            if let Some(begin) = start {
                if due.duration_since(begin) >= wall_time {
                    session.stop();
                    return Ok(counts.row(period, seconds));
                }
                counts.push(snapshot);
            }
        }
        if start.is_none() && Instant::now() >= warmup {
            return Err("No OP sample with a valid working counter during warm-up".into());
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn is_op(snapshot: &Snapshot) -> bool {
    snapshot
        .slaves
        .first()
        .is_some_and(|slave| slave.state.is_some_and(|state| state & 0x1f == 8))
}

#[derive(Default)]
struct Counts {
    samples: u64,
    valid: u64,
    op: u64,
    bad_al: u64,
    previous: Option<Instant>,
    intervals: Vec<f64>,
    wkc: BTreeMap<i32, u64>,
    gpio: BTreeSet<u16>,
    phase_min: Option<u16>,
    phase_max: Option<u16>,
    phase_invalid: u64,
}

impl Counts {
    fn push(&mut self, snapshot: Snapshot) {
        self.samples += 1;
        self.op += u64::from(is_op(&snapshot));
        self.bad_al += u64::from(
            snapshot
                .slaves
                .first()
                .and_then(|slave| slave.al_code)
                .is_some_and(|code| code != 0),
        );
        if let Some(previous) = self.previous.replace(snapshot.captured) {
            self.intervals
                .push(snapshot.captured.duration_since(previous).as_secs_f64() * 1e6);
        }
        let Some(sample) = snapshot.pdo else { return };
        *self.wkc.entry(sample.wkc).or_default() += 1;
        let Some(raw) = sample.raw else { return };
        self.valid += 1;
        self.gpio.insert(raw as u16);
        let phase = (raw >> 16) as u16;
        if phase == 0xffff {
            self.phase_invalid += 1;
        } else {
            self.phase_min = Some(self.phase_min.map_or(phase, |old| old.min(phase)));
            self.phase_max = Some(self.phase_max.map_or(phase, |old| old.max(phase)));
        }
    }

    fn row(mut self, period: u64, seconds: u64) -> String {
        self.intervals.sort_unstable_by(f64::total_cmp);
        let percentile = |pct: usize| {
            let index = (self.intervals.len() * pct).div_ceil(100).saturating_sub(1);
            self.intervals.get(index).copied().unwrap_or_default()
        };
        let slots = (seconds * 1_000_000).div_ceil(period);
        let invalid = self.samples - self.valid;
        let missed = slots.saturating_sub(self.samples);
        let wkc = self
            .wkc
            .iter()
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>()
            .join(";");
        let gpio = self
            .gpio
            .iter()
            .map(|word| format!("{word:04X}"))
            .collect::<Vec<_>>()
            .join(";");
        let display_phase =
            |value: Option<u16>| value.map_or_else(String::new, |value| value.to_string());
        format!("{period},{seconds},{slots},{},{},{invalid},{},{},{:.3},{:.5},{:.5},{:.2},{:.2},{:.2},{wkc},{gpio},{},{},{}",
            self.samples, self.valid, self.op, self.bad_al, self.valid as f64 / seconds as f64,
            100.0 * invalid as f64 / self.samples as f64, 100.0 * missed as f64 / slots as f64,
            percentile(50), percentile(99), percentile(100), display_phase(self.phase_min),
            display_phase(self.phase_max), self.phase_invalid)
    }
}
