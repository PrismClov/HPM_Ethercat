use eframe::egui::{self, Color32};
use ethercat_monitor::{
    backend::{Backend, Session, Slave, Update},
    service::{
        esi,
        process::{self, DriveState, Entry, Event, Options},
    },
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::trace::{Display, History, Key, Recorded, Source, Watch};
use crate::trace_view::{Timeline, WatchEdit};
use crate::{al_state, FAULT, GOOD, WARN};

#[derive(Default)]
struct Sample {
    values: Vec<Option<u64>>,
    valid: bool,
    wkc: i32,
    expected: i32,
}

#[derive(Default)]
struct Updates {
    entries: Option<Vec<Entry>>,
    sample: Option<Sample>,
    notices: VecDeque<(String, bool)>,
    finished: Option<bool>,
    frames: VecDeque<Recorded>,
}

struct Worker {
    commands: SyncSender<String>,
    stop: Arc<AtomicBool>,
    updates: Arc<Mutex<Updates>>,
    thread: Option<JoinHandle<()>>,
    writable: bool,
    stopping: bool,
    sync: (u32, u16),
}

impl Worker {
    fn start(options: Options, ctx: &egui::Context) -> Self {
        let (commands, rx) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let updates = Arc::new(Mutex::new(Updates::default()));
        let (worker_stop, worker_updates, ctx) = (stop.clone(), updates.clone(), ctx.clone());
        let writable = options.writable;
        let sync = (options.period_us, options.assign_activate);
        let thread = thread::spawn(move || {
            let result = process::serve(
                options,
                rx,
                || worker_stop.load(Ordering::Relaxed),
                |event| {
                    let mut updates = worker_updates.lock().unwrap();
                    let rejected = matches!(&event, Event::Rejected(_));
                    match event {
                        Event::Layout(entries) => updates.entries = Some(entries),
                        Event::Sample {
                            values,
                            valid,
                            wkc,
                            expected,
                        } => {
                            updates.sample = Some(Sample {
                                values,
                                valid,
                                wkc,
                                expected,
                            });
                        }
                        Event::Notice(text) | Event::Rejected(text) => {
                            if updates.notices.len() >= 100 {
                                updates.notices.pop_front();
                            }
                            updates.notices.push_back((text, rejected));
                        }
                    }
                    ctx.request_repaint();
                },
                |cycle| {
                    let limit = (8 * 1024 * 1024
                        / (size_of::<Recorded>() + cycle.values.len() * size_of::<Option<u64>>()))
                    .clamp(16, 4096);
                    let frame = Recorded::from(cycle);
                    let mut updates = worker_updates.lock().unwrap();
                    while updates.frames.len() >= limit {
                        updates.frames.pop_front();
                    }
                    updates.frames.push_back(frame);
                },
            );
            let mut updates = worker_updates.lock().unwrap();
            updates.finished = Some(result.is_ok());
            updates.notices.push_back(match result {
                Ok(()) => ("PDO 会话已停止。".into(), false),
                Err(error) => (error, true),
            });
            ctx.request_repaint();
        });
        Self {
            commands,
            stop,
            updates,
            thread: Some(thread),
            writable,
            stopping: false,
            sync,
        }
    }

    fn stop(&mut self) {
        self.stopping = true;
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type EsiStatus = (String, bool, bool);

pub(super) struct BusLog {
    pub time: Duration,
    pub text: String,
    pub error: bool,
}

pub(super) struct Advanced {
    pub(super) backend: Backend,
    adapters: crate::adapter_picker::AdapterPicker,
    pub(super) target: String,
    pub(super) cycle_us: u32,
    pub(super) dc: bool,
    pub(super) activation: String,
    scan: Option<Session>,
    scan_stopping: bool,
    next_start: Option<Options>,
    slaves: Vec<Slave>,
    pub(super) selected: Option<u32>,
    worker: Option<Worker>,
    entries: Vec<Entry>,
    sample: Sample,
    pub(super) watch_edit: Option<WatchEdit>,
    mode: i8,
    pub(super) logs: VecDeque<BusLog>,
    log_origin: Instant,
    pub(super) session_started: Duration,
    last_bus_state: Option<(bool, bool)>,
    esi_page: bool,
    xml: String,
    backup: String,
    ask_burn: bool,
    esi_job: Option<(JoinHandle<()>, Arc<Mutex<EsiStatus>>)>,
    demo_sii: Vec<u16>,
    pub(super) history: History,
    pub(super) watches: Vec<Watch>,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            backend: Backend::Demo,
            adapters: crate::adapter_picker::AdapterPicker::default(),
            target: String::new(),
            cycle_us: 5000,
            dc: false,
            activation: "0x0300".into(),
            scan: None,
            scan_stopping: false,
            next_start: None,
            slaves: Vec::new(),
            selected: None,
            worker: None,
            entries: Vec::new(),
            sample: Sample::default(),
            watch_edit: None,
            mode: 8,
            logs: VecDeque::new(),
            log_origin: Instant::now(),
            session_started: Duration::ZERO,
            last_bus_state: None,
            esi_page: false,
            xml: String::new(),
            backup: String::new(),
            ask_burn: false,
            esi_job: None,
            demo_sii: Vec::new(),
            history: History::default(),
            watches: Vec::new(),
        }
    }
}

impl Drop for Advanced {
    fn drop(&mut self) {
        if let Some((job, _)) = self.esi_job.take() {
            let _ = job.join();
        }
    }
}

pub(super) fn number(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

impl Advanced {
    pub(super) fn busy(&self) -> bool {
        self.scan.is_some() || self.worker.is_some() || self.esi_job.is_some() || self.ask_burn
    }

    pub(super) fn demo(&mut self, ctx: &egui::Context) {
        self.backend = Backend::Demo;
        self.dc = false;
        self.demo_scan();
        self.selected = Some(0);
        self.start(
            Options {
                backend: Backend::Demo,
                target: String::new(),
                period_us: self.cycle_us,
                assign_activate: 0,
                writable: true,
            },
            ctx,
        );
    }

    fn demo_scan(&mut self) {
        self.slaves = ["HPM5E31-KIT（模拟）", "CiA402 伺服（模拟）"]
            .into_iter()
            .enumerate()
            .map(|(position, name)| Slave {
                position: position as u32,
                name: name.into(),
                vendor: 0x24121,
                product: position as u32 + 1,
                revision: 1,
                state: Some(2),
                al_code: Some(0),
            })
            .collect();
        self.selected = Some(0);
        self.log("模拟总线 · 扫描完成，发现 2 台从站。".into(), false);
    }

    fn start(&mut self, options: Options, ctx: &egui::Context) {
        self.history = History::default();
        self.clear_layout();
        self.sample = Sample::default();
        self.session_started = self.log_origin.elapsed();
        self.last_bus_state = None;
        let mode = if options.assign_activate == 0 {
            "FreeRun"
        } else {
            "DC SYNC0"
        };
        self.log(
            format!("{} · {mode} · 正在建立过程映像…", options.backend.label()),
            false,
        );
        self.worker = Some(Worker::start(options, ctx));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(scan) = &self.scan {
            let mut stopped = false;
            for item in scan.take_updates() {
                match item {
                    Update::Snapshot { data, .. } if !self.scan_stopping => {
                        self.slaves = data.slaves;
                        if !self
                            .slaves
                            .iter()
                            .any(|s| Some(s.position) == self.selected)
                        {
                            self.selected = self.slaves.first().map(|s| s.position);
                        }
                        self.log_changed(
                            format!("总线报告 {} 台从站 · {}", data.count, data.status),
                            false,
                        );
                    }
                    Update::Error(error) => {
                        self.slaves.clear();
                        self.selected = None;
                        self.next_start = None;
                        self.log(error, true);
                    }
                    Update::Stopped => stopped = true,
                    _ => {}
                }
            }
            if stopped {
                self.scan = None;
                self.scan_stopping = false;
                if let Some(options) = self.next_start.take() {
                    if !self.slaves.is_empty() {
                        self.start(options, ctx);
                    }
                }
            }
        }
        if let Some(worker) = &self.worker {
            let mut frames;
            let layout;
            let notices;
            let finished = {
                let mut updates = worker.updates.lock().unwrap();
                layout = updates.entries.take();
                if let Some(sample) = updates.sample.take() {
                    self.sample = sample;
                }
                notices = std::mem::take(&mut updates.notices);
                frames = std::mem::take(&mut updates.frames);
                updates.finished
            };
            for (text, error) in notices {
                self.log(text, error);
            }
            if let Some(entries) = layout {
                self.history.layout(&entries);
                self.install_layout(entries);
            }
            while let Some(frame) = frames.pop_front() {
                self.history.push(frame);
            }
            if let Some(clean) = finished {
                self.worker = None;
                self.sample = Sample::default();
                if let Some(options) = self.next_start.take() {
                    if clean {
                        self.start(options, ctx);
                    }
                }
            }
        }
        if let Some((_, status)) = &self.esi_job {
            let (text, error, done) = status.lock().unwrap().clone();
            self.log_changed(text, error);
            if done {
                let (job, _) = self.esi_job.take().unwrap();
                let _ = job.join();
            }
        }
    }

    fn target_valid(&self) -> bool {
        let text = self.target.trim();
        match self.backend {
            Backend::Demo => true,
            Backend::Igh => cfg!(target_os = "linux") && text.parse::<u32>().is_ok(),
            Backend::Soem => {
                !text.is_empty() && !text.starts_with('-') && text != r"\Device\NPF_{GUID}"
            }
        }
    }

    pub(super) fn writable(&self) -> bool {
        self.history.cursor.is_none()
            && self.sample.valid
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.writable && !worker.stopping)
    }

    pub(super) fn write_observed_output(&mut self, key: &Key, value: u64) {
        if self.writable() && key.output && key.source == Source::Pdo {
            if let Some(id) = key.bind(&self.entries) {
                self.send(format!("set {id} {value}"));
            }
        }
    }

    fn send(&mut self, command: String) {
        if let Some(worker) = &self.worker {
            if let Err(error) = worker.commands.try_send(command) {
                self.log(format!("无法提交操作：{error}"), true);
            }
        }
    }

    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        self.log_bus_state();
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("连接设置", |ui| self.connection_settings(ui))
                .response
                .on_hover_text("选择主站、网卡和同步参数");
            let target_valid = self.target_valid();
            if ui
                .add_enabled(
                    !self.busy() && target_valid,
                    egui::Button::new("扫描从站").min_size(egui::vec2(90.0, 31.0)),
                )
                .on_hover_text("扫描前请确保本工具独占 EtherCAT 总线。")
                .clicked()
            {
                self.clear_layout();
                if self.backend == Backend::Demo {
                    self.demo_scan();
                } else {
                    self.slaves.clear();
                    self.selected = None;
                    let ctx = ui.ctx().clone();
                    self.scan = Some(Session::start(
                        self.backend,
                        self.target.trim().into(),
                        Duration::from_millis(350),
                        false,
                        move || ctx.request_repaint(),
                    ));
                }
            }
            let config = self.options(true);
            let can_start = target_valid
                && !self.slaves.is_empty()
                && self.worker.is_none()
                && self.esi_job.is_none()
                && self.next_start.is_none()
                && !self.scan_stopping
                && !self.ask_burn
                && config.is_some();
            let running =
                self.worker.as_ref().is_some_and(|worker| !worker.stopping) && self.sample.valid;
            let start = egui::Button::new(if running {
                "● PDO 运行中"
            } else {
                "启动 PDO"
            })
            .min_size(egui::vec2(105.0, 31.0));
            if running {
                ui.add(
                    start
                        .fill(Color32::from_rgb(23, 76, 65))
                        .sense(egui::Sense::hover()),
                );
            } else if ui
                .add_enabled(can_start, start)
                .on_hover_text("操作前请确保本工具独占 EtherCAT 总线。")
                .clicked()
            {
                let config = config.unwrap();
                if self.scan.is_some() {
                    self.next_start = Some(config);
                    self.stop_scan();
                } else {
                    self.start(config, ui.ctx());
                }
            }
            let can_stop = self.scan.is_some() && !self.scan_stopping
                || self.worker.as_ref().is_some_and(|worker| !worker.stopping);
            if ui
                .add_enabled(
                    can_stop,
                    egui::Button::new("停止").min_size(egui::vec2(72.0, 31.0)),
                )
                .clicked()
            {
                self.next_start = None;
                if let Some(worker) = self.worker.as_mut() {
                    worker.stop();
                } else {
                    self.stop_scan();
                }
            }
            ui.separator();
            ui.weak(self.backend.label());
            if self.backend != Backend::Demo && !self.target.is_empty() {
                ui.add(egui::Label::new("接口已选").sense(egui::Sense::hover()))
                    .on_hover_text(&self.target);
            }
        });
    }

    fn connection_settings(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(245.0);
        ui.heading("主站配置");
        ui.add_enabled_ui(!self.busy(), |ui| {
            let old = self.backend;
            egui::ComboBox::from_id_salt("advanced_backend")
                .selected_text(self.backend.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.backend, Backend::Demo, "演示");
                    ui.selectable_value(&mut self.backend, Backend::Igh, "IgH");
                    ui.selectable_value(&mut self.backend, Backend::Soem, "SOEM");
                });
            if self.backend != old {
                self.target = self.backend.default_target().into();
                if self.backend == Backend::Soem {
                    self.adapters.refresh(ui.ctx());
                }

                self.slaves.clear();
                self.clear_layout();
                self.selected = None;

                self.history = History::default();
            }
            let changed = match self.backend {
                Backend::Igh => {
                    ui.label("主站编号");
                    ui.add(egui::TextEdit::singleline(&mut self.target).desired_width(52.0))
                        .changed()
                }
                Backend::Soem => self.adapters.show(ui, &mut self.target),
                Backend::Demo => false,
            };
            if changed {
                self.slaves.clear();
                self.selected = None;
                self.clear_layout();

                self.history = History::default();
            }
        });
        ui.add_enabled_ui(
            self.esi_job.is_none()
                && !self.ask_burn
                && self.next_start.is_none()
                && !self.scan_stopping
                && !self.worker.as_ref().is_some_and(|w| w.stopping),
            |ui| {
                egui::ComboBox::from_id_salt("advanced_sync")
                    .selected_text(if self.dc { "DC SYNC0" } else { "FreeRun" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.dc, false, "FreeRun");
                        ui.selectable_value(&mut self.dc, true, "DC SYNC0");
                    });
                if self.dc {
                    ui.label("AssignActivate");
                    ui.add(egui::TextEdit::singleline(&mut self.activation).desired_width(63.0));
                }
                ui.horizontal(|ui| {
                    ui.label("频率");
                    let mut hz = 1_000_000.0 / f64::from(self.cycle_us);
                    if ui
                        .add(
                            egui::DragValue::new(&mut hz)
                                .range(50.0..=10_000.0)
                                .max_decimals(2)
                                .update_while_editing(false)
                                .suffix(" Hz"),
                        )
                        .changed()
                    {
                        self.cycle_us = (1_000_000.0 / hz).round() as u32;
                    }
                    egui::ComboBox::from_id_salt("frequency_presets")
                        .selected_text("预设")
                        .width(52.0)
                        .show_ui(ui, |ui| {
                            for preset in [50, 100, 200, 250, 500, 1000, 2000, 5000, 10_000] {
                                ui.selectable_value(
                                    &mut self.cycle_us,
                                    1_000_000 / preset,
                                    format!("{preset} Hz"),
                                );
                            }
                        });
                });
            },
        );
        let target_valid = self.target_valid();
        let writable = self.worker.as_ref().is_some_and(|worker| worker.writable);
        if let Some(config) = self.options(writable) {
            if let Some(worker) = self.worker.as_mut() {
                if worker.sync != (config.period_us, config.assign_activate)
                    && ui
                        .add_enabled(
                            !worker.stopping && target_valid,
                            egui::Button::new("应用并重连"),
                        )
                        .on_hover_text("停止当前会话并按新设置启动，输出将恢复为零。")
                        .clicked()
                {
                    self.next_start = Some(config);
                    worker.stop();
                    ui.close_menu();
                }
            }
        } else {
            ui.colored_label(WARN, "请核对 SYNC0 的 AssignActivate");
        }
        if ui.button("完成").clicked() {
            ui.close_menu();
        }
    }

    fn stop_scan(&mut self) {
        if let Some(scan) = &self.scan {
            scan.stop();
        }
        self.scan_stopping = true;
    }

    fn options(&self, writable: bool) -> Option<Options> {
        let assign_activate = if self.dc {
            let value = u16::try_from(number(&self.activation)?).ok()?;
            if value & 0x0700 != 0x0300 || self.backend == Backend::Soem && value != 0x0300 {
                return None;
            }
            value
        } else {
            0
        };
        Some(Options {
            backend: self.backend,
            target: self.target.trim().into(),
            period_us: self.cycle_us,
            assign_activate,
            writable,
        })
    }

    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("从站");
        if self.slaves.is_empty() {
            ui.weak("点击「扫描从站」");
        }
        for slave in &self.slaves {
            ui.horizontal(|ui| {
                let title = format!("{:02}  {}", slave.position, slave.name);
                if ui
                    .selectable_label(self.selected == Some(slave.position), title)
                    .clicked()
                {
                    self.selected = Some(slave.position);

                    self.ask_burn = false;
                }
                let state = if self.worker.is_some() {
                    self.history
                        .frames
                        .back()
                        .and_then(|frame| frame.al.iter().find(|al| al.station == slave.position))
                        .and_then(|al| al.state)
                } else {
                    slave.state
                };
                let (status, color) = al_state(state);
                ui.colored_label(color, status);
            });
        }
        if let Some(slave) = self
            .slaves
            .iter()
            .find(|s| Some(s.position) == self.selected)
        {
            ui.add_space(6.0);
            ui.separator();
            ui.weak("厂商 / 产品");
            ui.monospace(format!("{:08X} / {:08X}", slave.vendor, slave.product));
            ui.weak(format!("修订 {:08X}", slave.revision));
            if let Some(code) = slave.al_code {
                ui.weak(format!("AL 状态码 0x{code:04X}"));
            }
        }
    }

    pub(super) fn esi_tab(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                !self.ask_burn,
                egui::Button::new("ESI 烧录").selected(self.esi_page),
            )
            .clicked()
        {
            self.esi_page = !self.esi_page;
        }
    }

    pub(super) fn central(&mut self, ui: &mut egui::Ui, timeline: &mut Timeline) {
        if self.esi_page {
            egui::ScrollArea::both()
                .id_salt("advanced_detail")
                .show(ui, |ui| self.esi_panel(ui));
        } else {
            self.process_controls(ui);
            timeline.show(self, ui);
        }
    }

    fn log(&mut self, text: String, error: bool) {
        if self.logs.len() >= 100 {
            self.logs.pop_front();
        }
        self.logs.push_back(BusLog {
            time: self.log_origin.elapsed(),
            text,
            error,
        });
    }

    fn log_changed(&mut self, text: String, error: bool) {
        if self
            .logs
            .back()
            .is_none_or(|entry| entry.text != text || entry.error != error)
        {
            self.log(text, error);
        }
    }

    fn log_bus_state(&mut self) {
        let Some(worker) = &self.worker else {
            self.last_bus_state = None;
            return;
        };
        let state = (worker.stopping, self.sample.valid);
        if self.last_bus_state == Some(state) {
            return;
        }
        self.last_bus_state = Some(state);
        let label = match state {
            (true, _) => "正在释放主站",
            (false, true) => "OP · 有效 PDO",
            (false, false) => "等待有效 PDO",
        };
        let wkc = if self.sample.expected >= 0 {
            format!("WKC {} / {}", self.sample.wkc, self.sample.expected)
        } else {
            format!("WKC {}", self.sample.wkc)
        };
        self.log(format!("{label} · {wkc}"), false);
    }

    fn process_controls(&mut self, ui: &mut egui::Ui) {
        if self.backend == Backend::Demo
            && self.worker.is_some()
            && ui.small_button("模拟 WKC 归零").clicked()
        {
            self.send("demo wkc".into());
        }
        if let Some(station) = self.selected {
            self.drive_panel(ui, station, self.writable());
        }
    }

    fn clear_layout(&mut self) {
        self.entries.clear();
        self.watch_edit = None;
        self.watches.retain(|watch| !watch.is_default_mapping());
        for watch in &mut self.watches {
            watch.id = None;
        }
    }

    fn install_layout(&mut self, entries: Vec<Entry>) {
        self.clear_layout();
        for watch in &mut self.watches {
            watch.id = watch.key.bind(&entries);
        }
        for (id, entry) in entries.iter().enumerate() {
            let key = Key::entry(entry, Source::Pdo);
            if !self.watches.iter().any(|watch| watch.key == key) {
                self.push_watch(key, Some(id), false, true);
            }
        }
        self.entries = entries;
    }

    pub(super) fn can_split(&self, key: &Key) -> bool {
        key.source == Source::Pdo
            && !key.output
            && (key.index, key.sub, key.width) == (0x6000, 0, 32)
            && self.slaves.iter().any(|slave| {
                slave.position == key.station && (slave.vendor, slave.product) == (0x00024121, 1)
            })
    }

    pub(super) fn add_watch(&mut self, key: Key, id: Option<usize>, plotted: bool) {
        if let Some(watch) = self.watches.iter_mut().find(|watch| watch.key == key) {
            watch.plotted |= plotted;
            watch.id = id;
        } else {
            self.push_watch(key, id, plotted, false);
        }
    }

    fn push_watch(&mut self, key: Key, id: Option<usize>, plotted: bool, automatic: bool) {
        let display = if matches!(key.source, Source::Wkc | Source::Pdi) {
            Display::Decimal
        } else {
            Display::Hex
        };
        let group = if automatic {
            Watch::station_group(key.station)
        } else {
            "默认".into()
        };
        self.watches.push(Watch {
            key,
            id,
            plotted,
            display,
            group,
            color: self.watches.len() % 8,
            automatic,
        });
    }

    fn drive_panel(&mut self, ui: &mut egui::Ui, station: u32, writable: bool) {
        let find = |index, output, width| {
            let mut entries = self.entries.iter().enumerate().filter(|(_, e)| {
                (e.station, e.index, e.output, e.sub, e.width) == (station, index, output, 0, width)
            });
            let id = entries.next()?.0;
            entries.next().is_none().then_some(id)
        };
        let (Some(_), Some(status)) = (find(0x6040, true, 16), find(0x6041, false, 16)) else {
            return;
        };
        let state = self
            .sample
            .values
            .get(status)
            .copied()
            .flatten()
            .map(|word| process::drive_state(word as u16));
        let feedback = find(0x6061, false, 8)
            .and_then(|id| self.sample.values.get(id).copied().flatten())
            .map(|mode| mode as i8);
        let can_set_mode =
            find(0x6060, true, 8).is_some() && state.is_some_and(|v| v != DriveState::Enabled);
        egui::CollapsingHeader::new("CiA402 驱动控制").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("驱动状态");
                ui.colored_label(
                    if state == Some(DriveState::Fault) {
                        FAULT
                    } else {
                        GOOD
                    },
                    state.map_or("—", drive_label),
                );
                ui.label(feedback.map_or("模式反馈 —".into(), |mode| format!("模式反馈 {mode}")));
            });
            ui.horizontal_wrapped(|ui| {
                for (label, command, allowed) in [
                    (
                        "一键使能",
                        "enable",
                        matches!(
                            state,
                            Some(DriveState::Disabled | DriveState::Ready | DriveState::SwitchedOn)
                        ),
                    ),
                    ("故障复位", "reset", state == Some(DriveState::Fault)),
                    ("快速停止", "stop", state.is_some()),
                    ("禁止电压", "disable", state.is_some()),
                ] {
                    if ui
                        .add_enabled(writable && allowed, egui::Button::new(label))
                        .clicked()
                    {
                        self.send(format!("drive {station} {command}"));
                    }
                }
                if self.backend == Backend::Demo
                    && ui
                        .add_enabled(writable, egui::Button::new("模拟故障"))
                        .clicked()
                {
                    self.send("demo fault".into());
                }
            });
            ui.horizontal(|ui| {
                ui.label("运行模式");
                ui.add(egui::DragValue::new(&mut self.mode));
                if ui
                    .add_enabled(writable && can_set_mode, egui::Button::new("设置模式"))
                    .clicked()
                {
                    self.send(format!("drive {station} mode {}", self.mode));
                }
            });
        });
    }

    fn esi_panel(&mut self, ui: &mut egui::Ui) {
        if self.backend == Backend::Demo {
            ui.heading("ESI 示例 · HPM5E31-KIT");
            ui.weak("预览和写入使用内存中的模拟 EEPROM。");
            ui.horizontal(|ui| {
                for (caption, write) in [("预览变更", false), ("模拟烧录", true)] {
                    if ui.button(caption).clicked() {
                        let (text, error) = match esi::demo_update(&mut self.demo_sii, write) {
                            Ok(words) => (
                                if write {
                                    format!("模拟烧录并校验完成，更新 {words} 字。")
                                } else {
                                    format!("ESI 预览：{words} 个 SII 字将变化。")
                                },
                                false,
                            ),
                            Err(error) => (error, true),
                        };
                        self.log(text, error);
                    }
                }
            });
            return;
        }
        ui.heading("更新从站 EEPROM");
        let position = self.selected.and_then(|v| u16::try_from(v).ok());
        if let Some(position) = position {
            ui.label(format!("目标从站 {position}"));
        } else {
            ui.weak("先扫描并选择目标从站。");
        }
        ui.weak("先停止扫描和 PDO。每次操作使用新的备份路径。");
        ui.add_enabled_ui(self.esi_job.is_none() && !self.ask_burn, |ui| {
            esi_path(ui, "ESI XML 路径", &mut self.xml, false);
            esi_path(ui, "SII 备份路径", &mut self.backup, true);
        });
        let Some(position) = position else {
            return;
        };
        let ready = self.target_valid()
            && self.next_start.is_none()
            && self.scan.is_none()
            && self.worker.is_none()
            && self.esi_job.is_none()
            && !self.ask_burn
            && !self.xml.trim().is_empty()
            && !self.backup.trim().is_empty();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new("预览并备份"))
                .on_hover_text("操作前请确保本工具独占 EtherCAT 总线。")
                .clicked()
            {
                self.start_esi(position, false, ui.ctx());
            }
            if ui
                .add_enabled(ready, egui::Button::new("烧录并校验…"))
                .clicked()
            {
                self.ask_burn = true;
            }
        });
        if self.ask_burn {
            let mut action = None;
            egui::Window::new("确认写入 EEPROM")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!("将从站 {position} 的 PDO 类别写入 EEPROM。"));
                    ui.label("确保本工具独占总线，并核对目标和备份路径。");
                    ui.horizontal(|ui| {
                        if ui.button("取消").clicked() {
                            action = Some(false);
                        }
                        if ui.button("确认烧录").clicked() {
                            action = Some(true);
                        }
                    });
                });
            if let Some(write) = action {
                self.ask_burn = false;
                if write {
                    self.start_esi(position, true, ui.ctx());
                }
            }
        }
    }

    fn start_esi(&mut self, position: u16, write: bool, ctx: &egui::Context) {
        let (backend, target, xml, backup, ctx) = (
            self.backend,
            self.target.trim().to_owned(),
            self.xml.trim().to_owned(),
            self.backup.trim().to_owned(),
            ctx.clone(),
        );
        let status = Arc::new(Mutex::new(("正在读取 SII…".into(), false, false)));
        let shared = status.clone();
        let job = thread::spawn(move || {
            let operation =
                esi::run_with_progress(backend, &target, position, &xml, &backup, write, |text| {
                    *shared.lock().unwrap() = (text, false, false);
                    ctx.request_repaint();
                });
            let mut status = shared.lock().unwrap();
            status.0 = match operation {
                Ok(()) if write => format!("EEPROM 操作完成 · {}", status.0),
                Ok(()) => format!("预览完成 · {}", status.0),
                Err(error) => {
                    status.1 = true;
                    error
                }
            };
            status.2 = true;
            ctx.request_repaint();
        });
        self.esi_job = Some((job, status));
    }
}

fn esi_path(ui: &mut egui::Ui, label: &str, value: &mut String, backup: bool) {
    ui.label(label);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(360.0)
                .background_color(ui.visuals().panel_fill)
                .hint_text(if backup {
                    "选择新的备份文件"
                } else {
                    "选择本机 XML 文件"
                }),
        );
        if ui
            .button(if backup { "另存为…" } else { "浏览…" })
            .clicked()
        {
            let dialog = if backup {
                rfd::FileDialog::new()
                    .set_title("选择新的 SII 备份路径")
                    .add_filter("SII 备份", &["sii"])
                    .set_file_name("backup.sii")
            } else {
                rfd::FileDialog::new()
                    .set_title("选择 ESI XML 文件")
                    .add_filter("ESI XML", &["xml"])
            };
            let path = if backup {
                dialog.save_file()
            } else {
                dialog.pick_file()
            };
            if let Some(mut path) = path {
                if backup && path.extension().is_none() {
                    path.set_extension("sii");
                }
                *value = path.to_string_lossy().into_owned();
            }
        }
    });
}

fn drive_label(state: DriveState) -> &'static str {
    match state {
        DriveState::NotReady => "未就绪",
        DriveState::Disabled => "禁止使能",
        DriveState::Ready => "上电就绪",
        DriveState::SwitchedOn => "已开通",
        DriveState::Enabled => "运行使能",
        DriveState::QuickStop => "快速停止",
        DriveState::Reaction => "故障反应",
        DriveState::Fault => "故障",
        DriveState::Unknown => "未知",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_pdos_appear_in_the_sidebar_and_keep_waveform_settings() {
        let mut app = Advanced::default();
        app.demo_scan();
        let entry = Entry {
            station: 0,
            pdo: 0x1a00,
            index: 0x6000,
            sub: 0,
            width: 32,
            offset: 0,
            output: false,
        };
        let key = Key::entry(&entry, Source::Pdo);
        app.install_layout(vec![entry.clone()]);

        assert_eq!(app.watches.len(), 1);
        assert_eq!(app.watches[0].key, key);
        assert_eq!(app.watches[0].id, Some(0));
        assert!(app.can_split(&key));
        assert!(app.watches[0].is_default_mapping());

        app.add_watch(key.clone(), Some(0), true);
        app.install_layout(vec![entry]);

        assert_eq!(app.watches.len(), 1);
        assert_eq!(app.watches[0].key, key);
        assert!(app.watches[0].plotted);
        assert!(!app.watches[0].is_default_mapping());
    }
}
