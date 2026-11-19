use std::{
    collections::VecDeque,
    env,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
pub enum Backend {
    Demo,
    Igh,
    Soem,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Demo => "演示",
            Self::Igh => "IgH",
            Self::Soem => "SOEM",
        }
    }

    pub fn default_target(self) -> &'static str {
        match self {
            Self::Demo => "",
            Self::Igh => "0",
            Self::Soem => "",
        }
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Igh => "ecat-monitor-igh",
            Self::Soem => "ecat-monitor-soem",
            Self::Demo => unreachable!(),
        }
    }
}

pub struct Slave {
    pub position: u32,
    pub name: String,
    pub vendor: u32,
    pub product: u32,
    pub revision: u32,
    pub state: Option<u16>,
    pub al_code: Option<u16>,
}

pub struct Snapshot {
    pub count: u32,
    pub status: String,
    pub slaves: Vec<Slave>,
    pub captured: Instant,
    pub pdo: Option<PdoSample>,
}

#[derive(Clone, Copy)]
pub struct PdoSample {
    pub raw: Option<u32>,
    pub wkc: i32,
    pub expected_wkc: Option<u32>,
}

pub enum Update {
    Snapshot { due: Instant, data: Snapshot },
    Error(String),
    Stopped,
}

pub struct Session {
    updates: Arc<Mutex<VecDeque<Update>>>,
    stop: Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Session {
    pub fn start(
        backend: Backend,
        target: String,
        interval: Duration,
        pdo: bool,
        on_update: impl Fn() + Send + 'static,
    ) -> Self {
        let updates = Arc::new(Mutex::new(VecDeque::new()));
        let worker_updates = Arc::clone(&updates);
        let (stop, stop_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = (|| {
                let mut source = match backend {
                    Backend::Demo => Source::Demo(0),
                    _ => Source::Native(Helper::start(backend, &target, pdo)?),
                };
                let mut last_repaint = None;
                let mut status_at = Instant::now();
                let mut due = status_at;
                loop {
                    if !matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        break;
                    }
                    let started = Instant::now();
                    let read_status = started >= status_at;
                    if read_status {
                        status_at = started + Duration::from_millis(250);
                    }
                    let snapshot = source.snapshot(&stop_rx, pdo, read_status)?;
                    if pdo && snapshot.pdo.is_none() {
                        return Err("后端未返回 PDO，请重新构建或更换后端程序".into());
                    }
                    {
                        let mut queue = worker_updates.lock().expect("update queue");
                        if queue.len() == 1024 {
                            queue.pop_front();
                        }
                        queue.push_back(Update::Snapshot {
                            due,
                            data: snapshot,
                        });
                    }
                    let now = Instant::now();
                    if last_repaint.is_none_or(|last| now - last >= Duration::from_millis(33)) {
                        on_update();
                        last_repaint = Some(now);
                    }
                    if pdo {
                        due = next_slot(due, now, interval);
                        if !wait_for_slot(&stop_rx, due, interval) {
                            break;
                        }
                    } else if !matches!(
                        stop_rx.recv_timeout(interval.saturating_sub(started.elapsed())),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
                Ok(())
            })();
            {
                let mut queue = worker_updates.lock().expect("update queue");
                if let Err(error) = result {
                    queue.push_back(Update::Error(error));
                }
                queue.push_back(Update::Stopped);
            }
            on_update();
        });
        Self {
            updates,
            stop,
            worker: Some(worker),
        }
    }

    pub fn take_updates(&self) -> VecDeque<Update> {
        std::mem::take(&mut *self.updates.lock().expect("update queue"))
    }

    pub fn stop(&self) {
        let _ = self.stop.send(());
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn next_slot(previous: Instant, now: Instant, interval: Duration) -> Instant {
    let elapsed = now.saturating_duration_since(previous);
    let remainder = elapsed.as_nanos() % interval.as_nanos();
    now + (interval - Duration::from_nanos(remainder as u64))
}

fn wait_for_slot(stop: &Receiver<()>, due: Instant, interval: Duration) -> bool {
    // High-rate PDO periods trade CPU for lower scheduling jitter.
    let spin = if interval <= Duration::from_millis(2) {
        interval
    } else {
        Duration::from_micros(200)
    };
    loop {
        if !matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            return false;
        }
        let remaining = due.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        if remaining > spin {
            thread::sleep((remaining - spin).min(Duration::from_millis(2)));
        } else {
            std::hint::spin_loop();
        }
    }
}

enum Source {
    Demo(usize),
    Native(Helper),
}

impl Source {
    fn snapshot(
        &mut self,
        stop: &Receiver<()>,
        pdo: bool,
        read_status: bool,
    ) -> Result<Snapshot, String> {
        match self {
            Self::Demo(tick) => {
                *tick = tick.wrapping_add(1);
                let fault = *tick % 16 >= 12;
                Ok(Snapshot {
                    count: 3,
                    status: "demo".into(),
                    slaves: ["HPM5E31-KIT (demo)", "Digital I/O (demo)", "Slave C (demo)"]
                        .iter()
                        .enumerate()
                        .map(|(position, name)| Slave {
                            position: position as u32,
                            name: (*name).into(),
                            vendor: 0x1000,
                            product: position as u32 + 1,
                            revision: 1,
                            state: Some(if position == 2 && fault { 0x14 } else { 8 }),
                            al_code: Some(if position == 2 && fault { 0x001b } else { 0 }),
                        })
                        .collect(),
                    captured: Instant::now(),
                    pdo: pdo.then(|| {
                        let phase = (180.0 + 65.0 * (*tick as f64 / 80.0).sin()) as u32;
                        let phase = if *tick % 1000 < 50 { 0xffff } else { phase };
                        let gpio = ((*tick / 90) & 0xffff) as u32;
                        let valid = *tick % 1450 >= 25;
                        PdoSample {
                            raw: valid.then_some((phase << 16) | gpio),
                            wkc: if valid { 3 } else { 0 },
                            expected_wkc: Some(3),
                        }
                    }),
                })
            }
            Self::Native(helper) => helper.snapshot(stop, !pdo || read_status),
        }
    }
}

struct Helper {
    child: Child,
    input: Option<ChildStdin>,
    frames: Receiver<Vec<String>>,
    awaiting_first: bool,
    pdo: bool,
}

impl Helper {
    fn start(backend: Backend, target: &str, pdo: bool) -> Result<Self, String> {
        let exe = format!("{}{}", backend.executable(), env::consts::EXE_SUFFIX);
        let directory = env::var_os("ECAT_MONITOR_BACKEND_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or(
                env::current_exe()
                    .map_err(|e| e.to_string())?
                    .with_file_name(""),
            );
        let path = directory.join(exe);
        let mut command = Command::new(&path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command.arg(target);
        if pdo {
            command.arg("--pdo");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("启动 {} 失败：{e}", path.display()))?;
        let input = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, frames) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut frame = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // SOEM also writes unframed diagnostics on stdout.
                let Some(message) = line.strip_prefix("ecat\t") else {
                    continue;
                };
                let complete = message == "end" || message.starts_with("error\t");
                frame.push(message.to_owned());
                if (complete || frame.len() > 65_540)
                    && tx.send(std::mem::take(&mut frame)).is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input: Some(input),
            frames,
            awaiting_first: true,
            pdo,
        })
    }

    fn snapshot(&mut self, stop: &Receiver<()>, read_status: bool) -> Result<Snapshot, String> {
        // A helper can report an initialization error before reading our request.
        let request = self
            .input
            .as_mut()
            .expect("open stdin")
            .write_all(if read_status { b"S\n" } else { b"P\n" });
        let timeout = if self.awaiting_first || !self.pdo {
            Duration::from_secs(20)
        } else {
            Duration::from_secs(2)
        };
        let deadline = Instant::now() + timeout;
        let frame = loop {
            if !matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                return Err("已停止监测".into());
            }
            if Instant::now() >= deadline {
                return Err("后端响应超时".into());
            }

            match self.frames.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => break frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(match request {
                        Ok(()) => "后端进程意外退出".into(),
                        Err(error) => format!("后端请求失败：{error}"),
                    });
                }
            };
        };
        let mut snapshot = None;
        for message in frame {
            let fields: Vec<_> = message.split('\t').collect();
            match fields.as_slice() {
                ["error", message] => return Err((*message).into()),
                ["snapshot", count, status] if snapshot.is_none() => {
                    snapshot = Some(Snapshot {
                        count: decimal(count)?,
                        status: (*status).into(),
                        slaves: Vec::new(),
                        captured: Instant::now(),
                        pdo: None,
                    });
                }
                ["slave", position, vendor, product, revision, state, code, name] => {
                    let frame = snapshot.as_mut().ok_or("后端缺少快照头")?;
                    if frame.slaves.len() >= frame.count as usize {
                        return Err("后端返回的从站数量超出快照范围".into());
                    }
                    frame.slaves.push(Slave {
                        position: decimal(position)?,
                        vendor: hex(vendor)?,
                        product: hex(product)?,
                        revision: hex(revision)?,
                        state: protocol_word(state)?,
                        al_code: protocol_word(code)?,
                        name: decode_name(name)?,
                    });
                }
                ["pdo", wkc, expected, raw] => {
                    let frame = snapshot.as_mut().ok_or("后端缺少快照头")?;
                    if frame.pdo.is_some() {
                        return Err("后端返回了重复的 PDO 样本".into());
                    }
                    frame.pdo = Some(PdoSample {
                        raw: if raw.is_empty() {
                            None
                        } else {
                            Some(hex(raw)?)
                        },
                        wkc: wkc.parse().map_err(|_| "无效的 WKC")?,
                        expected_wkc: if *expected == "-1" {
                            None
                        } else {
                            Some(decimal(expected)?)
                        },
                    });
                }
                ["end"] => {
                    let mut frame = snapshot.ok_or("后端返回空快照")?;
                    frame.captured = Instant::now();
                    self.awaiting_first = false;
                    return Ok(frame);
                }
                _ => return Err("无法识别后端响应".into()),
            }
        }
        Err("后端返回了不完整的快照".into())
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // EOF allows SOEM to request SAFE-OP and IgH to release its domain.
        self.input.take();
        let deadline = Instant::now() + Duration::from_millis(600);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            let _ = self.frames.recv_timeout(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn decimal(value: &str) -> Result<u32, String> {
    value.parse().map_err(|_| "无效的后端十进制字段".into())
}

fn hex(value: &str) -> Result<u32, String> {
    u32::from_str_radix(value, 16).map_err(|_| "无效的后端十六进制字段".into())
}

fn protocol_word(value: &str) -> Result<Option<u16>, String> {
    if value == "-1" {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| "无效的后端状态字段".into())
    }
}

pub(crate) fn decode_name(value: &str) -> Result<String, String> {
    if !value.is_ascii() || value.len() % 2 != 0 {
        return Err("无效的后端名称字段".into());
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "无效的后端名称编码")?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdo_clock_stays_on_phase_and_skips_elapsed_slots() {
        let start = Instant::now();
        let period = Duration::from_micros(500);
        assert_eq!(
            next_slot(start, start + Duration::from_micros(90), period),
            start + period
        );
        assert_eq!(
            next_slot(start, start + Duration::from_micros(620), period),
            start + period * 2
        );
        assert_eq!(next_slot(start, start + period, period), start + period * 2);
    }
}
