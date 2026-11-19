use eframe::egui::{self, Color32, Frame, Margin};
use ethercat_monitor::{
    backend::{Backend, Session, Slave, Update},
    service::{
        esi,
        process::{self, DriveState, Entry, Event, Options},
    },
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

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
    notice: Option<(String, bool)>,
    finished: Option<bool>,
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
                        Event::Notice(text) => updates.notice = Some((text, false)),
                        Event::Rejected(text) => updates.notice = Some((text, true)),
                    }
                    ctx.request_repaint();
                },
            );
            let mut updates = worker_updates.lock().unwrap();
            updates.finished = Some(result.is_ok());
            updates.notice = Some(match result {
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

pub(super) struct Advanced {
    backend: Backend,
    adapters: crate::adapter_picker::AdapterPicker,
    target: String,
    confirmed: bool,
    writes: bool,
    cycle_us: u32,
    dc: bool,
    activation: String,
    scan: Option<Session>,
    scan_stopping: bool,
    next_start: Option<Options>,
    slaves: Vec<Slave>,
    selected: Option<u32>,
    worker: Option<Worker>,
    entries: Vec<Entry>,
    sample: Sample,
    write_id: Option<usize>,
    write_value: String,
    mode: i8,
    notice: Option<(String, bool)>,
    esi_page: bool,
    xml: String,
    backup: String,
    ask_burn: bool,
    esi_job: Option<(JoinHandle<()>, Arc<Mutex<EsiStatus>>)>,
    demo_sii: Vec<u16>,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            backend: Backend::Demo,
            adapters: crate::adapter_picker::AdapterPicker::default(),
            target: String::new(),
            confirmed: false,
            writes: false,
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
            write_id: None,
            write_value: "0".into(),
            mode: 8,
            notice: None,
            esi_page: false,
            xml: String::new(),
            backup: String::new(),
            ask_burn: false,
            esi_job: None,
            demo_sii: Vec::new(),
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

fn number(text: &str) -> Option<u64> {
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
        self.demo_scan();
        self.selected = Some(1);
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
        self.notice = Some(("演示扫描完成，发现 2 台模拟从站。".into(), false));
    }

    fn start(&mut self, options: Options, ctx: &egui::Context) {
        self.entries.clear();
        self.sample = Sample::default();
        self.write_id = None;
        self.notice = Some(("正在建立过程映像…".into(), false));
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
                        self.notice = Some((
                            format!("总线报告 {} 台从站 · {}", data.count, data.status),
                            false,
                        ));
                    }
                    Update::Error(error) => {
                        self.slaves.clear();
                        self.selected = None;
                        self.next_start = None;
                        self.notice = Some((error, true));
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
            let finished = {
                let mut updates = worker.updates.lock().unwrap();
                if let Some(entries) = updates.entries.take() {
                    self.entries = entries;
                }
                if let Some(sample) = updates.sample.take() {
                    self.sample = sample;
                }
                if let Some(notice) = updates.notice.take() {
                    self.notice = Some(notice);
                }
                updates.finished
            };
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
            self.notice = Some((text, error));
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

    fn writable(&self) -> bool {
        self.sample.valid
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.writable && !worker.stopping)
    }

    fn send(&mut self, command: String) {
        if let Some(worker) = &self.worker {
            if let Err(error) = worker.commands.try_send(command) {
                self.notice = Some((format!("无法提交操作：{error}"), true));
            }
        }
    }

    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        ui.horizontal_wrapped(|ui| {
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
                    self.confirmed = false;
                    self.writes = false;
                    self.slaves.clear();
                    self.entries.clear();
                    self.selected = None;
                    self.write_id = None;
                    self.notice = None;
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
                    self.confirmed = false;
                    self.slaves.clear();
                    self.selected = None;
                    self.entries.clear();
                    self.write_id = None;
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
                        ui.add(
                            egui::TextEdit::singleline(&mut self.activation).desired_width(63.0),
                        );
                    }
                    ui.add(
                        egui::DragValue::new(&mut self.cycle_us)
                            .range(100..=20_000)
                            .suffix(" µs"),
                    );
                },
            );
            let exclusive =
                self.target_valid() && (self.confirmed || self.backend == Backend::Demo);
            let config = self.options();
            if self.scan.is_some()
                && ui
                    .add_enabled(!self.scan_stopping, egui::Button::new("停止扫描"))
                    .clicked()
            {
                self.stop_scan();
            } else if self.scan.is_none()
                && ui
                    .add_enabled(!self.busy() && exclusive, egui::Button::new("扫描从站"))
                    .clicked()
            {
                self.entries.clear();
                self.write_id = None;
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
            if let Some(worker) = self.worker.as_mut() {
                if ui
                    .add_enabled(!worker.stopping, egui::Button::new("停止 PDO"))
                    .clicked()
                {
                    self.next_start = None;
                    worker.stop();
                }
                if let Some(config) = config {
                    let changed = worker.sync != (config.period_us, config.assign_activate);
                    if changed
                        && ui
                            .add_enabled(
                                !worker.stopping && exclusive,
                                egui::Button::new("应用并重连"),
                            )
                            .on_hover_text("停止当前会话并按新设置启动，输出将恢复为零。")
                            .clicked()
                    {
                        self.next_start = Some(config);
                        worker.stop();
                    }
                }
            } else {
                let can_start = exclusive
                    && !self.slaves.is_empty()
                    && self.esi_job.is_none()
                    && self.next_start.is_none()
                    && !self.scan_stopping
                    && !self.ask_burn
                    && config.is_some();
                if ui
                    .add_enabled(
                        can_start,
                        egui::Button::new("启动 PDO").fill(Color32::from_rgb(23, 76, 82)),
                    )
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
            }
        });
        if self.backend != Backend::Demo {
            ui.add_enabled_ui(!self.busy(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.confirmed, "确认总线已停机且由本工具独占");
                    ui.checkbox(&mut self.writes, "允许本次会话写入输出");
                });
            });
        }
    }

    fn stop_scan(&mut self) {
        if let Some(scan) = &self.scan {
            scan.stop();
        }
        self.scan_stopping = true;
    }

    fn options(&self) -> Option<Options> {
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
            writable: self.writes || self.backend == Backend::Demo,
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
                    self.write_id = None;
                    self.ask_burn = false;
                }
                if self.worker.is_some() {
                    if self.sample.valid {
                        ui.colored_label(GOOD, "OP");
                    }
                } else {
                    let (status, color) = al_state(slave.state);
                    ui.colored_label(color, status);
                }
            });
        }
        if let Some(slave) = self
            .slaves
            .iter()
            .find(|s| Some(s.position) == self.selected)
        {
            ui.add_space(6.0);
            ui.separator();
            ui.weak("厂商 / 产品 / 修订");
            ui.monospace(format!(
                "{:08X} / {:08X} / {:08X}",
                slave.vendor, slave.product, slave.revision
            ));
            if let Some(code) = slave.al_code {
                ui.weak(format!("AL 状态码 0x{code:04X}"));
            }
        }
    }

    pub(super) fn central(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(!self.ask_burn, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.esi_page, false, "过程映像");
                ui.selectable_value(&mut self.esi_page, true, "ESI 烧录");
            })
        });
        ui.add_space(7.0);
        egui::ScrollArea::both()
            .id_salt("advanced_detail")
            .show(ui, |ui| {
                if self.esi_page {
                    self.esi_panel(ui);
                } else {
                    self.process_panel(ui);
                }
            });
    }

    pub(super) fn status(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.backend == Backend::Demo {
                ui.colored_label(GOOD, "模拟总线");
            }
            if let Some(worker) = &self.worker {
                let (label, color) = if worker.stopping {
                    ("正在释放主站", WARN)
                } else if self.sample.valid {
                    ("OP · 有效 PDO", GOOD)
                } else {
                    ("等待有效 PDO", WARN)
                };
                ui.colored_label(color, label);
                ui.label(if worker.sync.1 == 0 {
                    "FreeRun"
                } else {
                    "DC SYNC0"
                });
                ui.label(if self.sample.expected >= 0 {
                    format!("WKC {} / {}", self.sample.wkc, self.sample.expected)
                } else {
                    format!("WKC {}", self.sample.wkc)
                });
            }
            if let Some((notice, error)) = &self.notice {
                ui.colored_label(
                    if *error {
                        FAULT
                    } else {
                        ui.visuals().text_color()
                    },
                    notice,
                );
            }
        });
    }

    fn process_panel(&mut self, ui: &mut egui::Ui) {
        let Some(station) = self.selected else {
            ui.weak("先扫描总线，再选择从站。");
            return;
        };
        ui.heading("在线 PDO");
        if self.entries.is_empty() {
            ui.weak("启动 PDO 后显示从站的 RxPDO 和 TxPDO。");
            return;
        }
        let writable = self.writable();
        egui::Grid::new("generic_pdos")
            .striped(true)
            .spacing(egui::vec2(17.0, 6.0))
            .show(ui, |ui| {
                for label in ["方向", "PDO", "对象", "位宽", "位偏移", "当前值", ""] {
                    ui.strong(label);
                }
                ui.end_row();
                for (id, entry) in self
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.station == station)
                {
                    ui.label(if entry.output {
                        "Rx · 输出"
                    } else {
                        "Tx · 输入"
                    });
                    ui.monospace(format!("0x{:04X}", entry.pdo));
                    ui.monospace(format!("0x{:04X}:{:02X}", entry.index, entry.sub));
                    ui.monospace(entry.width.to_string());
                    ui.monospace(entry.offset.to_string());
                    ui.monospace(
                        self.sample
                            .values
                            .get(id)
                            .copied()
                            .flatten()
                            .map_or("—".into(), |v| format!("0x{v:X}")),
                    );
                    if entry.output && entry.width <= 64 {
                        if ui
                            .add_enabled(
                                writable,
                                egui::Button::new("编辑").selected(self.write_id == Some(id)),
                            )
                            .clicked()
                        {
                            self.write_id = Some(id);
                            self.write_value = self
                                .sample
                                .values
                                .get(id)
                                .copied()
                                .flatten()
                                .unwrap_or(0)
                                .to_string();
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
        if let Some(id) = self.write_id {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("输出值");
                ui.add(egui::TextEdit::singleline(&mut self.write_value).desired_width(120.0));
                let value = number(&self.write_value);
                if ui
                    .add_enabled(writable && value.is_some(), egui::Button::new("写入 RxPDO"))
                    .clicked()
                {
                    self.send(format!("set {id} {}", value.unwrap()));
                }
            });
        }
        self.drive_panel(ui, station, writable);
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
        ui.add_space(12.0);
        Frame::group(ui.style())
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.heading("CiA402 驱动控制");
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
                    ui.label(
                        feedback
                            .map_or("模式反馈 —".into(), |mode| format!("模式反馈 {mode}")),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    for (label, command, allowed) in [
                        (
                            "一键使能",
                            "enable",
                            matches!(
                                state,
                                Some(
                                    DriveState::Disabled
                                        | DriveState::Ready
                                        | DriveState::SwitchedOn
                                )
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
                        self.notice = Some(match esi::demo_update(&mut self.demo_sii, write) {
                            Ok(words) => (
                                if write {
                                    format!("模拟烧录并校验完成，更新 {words} 字。")
                                } else {
                                    format!("ESI 预览：{words} 个 SII 字将变化。")
                                },
                                false,
                            ),
                            Err(error) => (error, true),
                        });
                    }
                }
            });
            return;
        }
        ui.heading("更新从站 EEPROM");
        let Some(position) = self.selected.and_then(|v| u16::try_from(v).ok()) else {
            ui.weak("先扫描并选择目标从站。");
            return;
        };
        ui.label(format!("目标从站 {position}"));
        ui.weak("先停止扫描和 PDO。每次操作使用新的备份路径。");
        ui.add_enabled_ui(self.esi_job.is_none() && !self.ask_burn, |ui| {
            for (label, value) in [
                ("ESI XML 路径", &mut self.xml),
                ("SII 备份路径", &mut self.backup),
            ] {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(value).desired_width(400.0));
            }
        });
        let ready = self.confirmed
            && self.target_valid()
            && self.scan.is_none()
            && self.worker.is_none()
            && self.esi_job.is_none()
            && !self.ask_burn
            && !self.xml.trim().is_empty()
            && !self.backup.trim().is_empty();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new("预览并备份"))
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
                    ui.label("核对目标和备份路径后继续。");
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
