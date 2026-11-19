#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod adapter_picker;
mod advanced;
mod pdo;
mod workbench;

use eframe::egui::{self, Color32};
use ethercat_monitor::backend::{Backend, Session, Snapshot, Update};
use std::{fs, time::Duration};

const GOOD: Color32 = Color32::from_rgb(98, 222, 183);
const WARN: Color32 = Color32::from_rgb(246, 193, 100);
const FAULT: Color32 = Color32::from_rgb(252, 127, 128);

struct Monitor {
    backend: Backend,
    target: String,
    interval_ms: u64,
    pdo_cycle_us: u64,
    pdo_enabled: bool,
    pdo_view: pdo::PdoView,
    bus_confirmed: bool,
    session: Option<Session>,
    stopping: bool,
    snapshot: Option<Snapshot>,
    error: Option<String>,
    selected_slave: Option<u32>,
    show_advanced: bool,
    advanced: advanced::Advanced,
    adapters: adapter_picker::AdapterPicker,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            backend: Backend::Demo,
            target: String::new(),
            interval_ms: 500,
            pdo_cycle_us: 5000,
            pdo_enabled: true,
            pdo_view: pdo::PdoView::default(),
            bus_confirmed: false,
            session: None,
            stopping: false,
            snapshot: None,
            error: None,
            selected_slave: None,
            show_advanced: true,
            advanced: advanced::Advanced::default(),
            adapters: adapter_picker::AdapterPicker::default(),
        }
    }
}

impl Monitor {
    fn receive_updates(&mut self) {
        let Some(session) = &self.session else { return };
        let mut stopped = false;
        for update in session.take_updates() {
            match update {
                Update::Snapshot { data: snapshot, .. } if !self.stopping => {
                    if let Some(sample) = snapshot.pdo {
                        self.pdo_view.push(snapshot.captured, sample);
                    }
                    self.snapshot = Some(snapshot);
                }
                Update::Error(error) if !self.stopping => self.error = Some(error),
                Update::Stopped => stopped = true,
                _ => {}
            }
        }
        if stopped {
            self.session = None;
            self.snapshot = None;
            self.stopping = false;
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let active = self.session.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!active, |ui| {
                let previous = self.backend;
                ui.label("主站");
                egui::ComboBox::from_id_salt("backend")
                    .selected_text(self.backend.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.backend, Backend::Demo, "演示");
                        ui.selectable_value(&mut self.backend, Backend::Igh, "IgH");
                        ui.selectable_value(&mut self.backend, Backend::Soem, "SOEM");
                    });
                if self.backend != previous {
                    self.target = self.backend.default_target().into();
                    if self.backend == Backend::Soem {
                        self.adapters.refresh(ui.ctx());
                    }
                    self.pdo_enabled = self.backend == Backend::Demo;
                    self.bus_confirmed = false;
                    self.error = None;
                    self.pdo_view = pdo::PdoView::default();
                }

                match self.backend {
                    Backend::Demo => {}
                    Backend::Igh => {
                        ui.label("主站编号");
                        if ui
                            .add(egui::TextEdit::singleline(&mut self.target).desired_width(52.0))
                            .changed()
                        {
                            self.bus_confirmed = false;
                        }
                    }
                    Backend::Soem => {
                        if self.adapters.show(ui, &mut self.target) {
                            self.bus_confirmed = false;
                        }
                    }
                }
                if ui.checkbox(&mut self.pdo_enabled, "PDO 采集").changed() {
                    self.bus_confirmed = false;
                    self.pdo_view = pdo::PdoView::default();
                }
                if self.pdo_enabled {
                    ui.label("采集周期");
                    ui.add(
                        egui::DragValue::new(&mut self.pdo_cycle_us)
                            .range(100..=20_000)
                            .speed(50.0)
                            .suffix(" µs"),
                    );
                } else {
                    ui.label("刷新间隔");
                    ui.add(
                        egui::DragValue::new(&mut self.interval_ms)
                            .range(100..=5000)
                            .suffix(" ms"),
                    );
                }
            });

            let target = self.target.trim();
            let valid = match self.backend {
                Backend::Demo => true,
                Backend::Igh => {
                    cfg!(target_os = "linux")
                        && target.parse::<u32>().is_ok()
                        && (!self.pdo_enabled || self.bus_confirmed)
                }
                Backend::Soem => {
                    self.bus_confirmed && !target.is_empty() && target != r"\Device\NPF_{GUID}"
                }
            };
            if let Some(session) = &self.session {
                if ui
                    .add_enabled(!self.stopping, egui::Button::new("断开"))
                    .clicked()
                {
                    self.stopping = true;
                    session.stop();
                }
            } else if ui
                .add_enabled(
                    valid,
                    egui::Button::new("连接").fill(Color32::from_rgb(23, 76, 82)),
                )
                .clicked()
            {
                self.error = None;
                self.selected_slave = None;
                self.pdo_view = pdo::PdoView::default();
                let repaint = ui.ctx().clone();
                self.session = Some(Session::start(
                    self.backend,
                    if self.backend == Backend::Igh {
                        target
                            .parse::<u32>()
                            .expect("validated master index")
                            .to_string()
                    } else {
                        target.to_owned()
                    },
                    if self.pdo_enabled {
                        Duration::from_micros(self.pdo_cycle_us)
                    } else {
                        Duration::from_millis(self.interval_ms)
                    },
                    self.pdo_enabled,
                    move || repaint.request_repaint(),
                ));
                self.bus_confirmed = false;
            }
        });
        if self.pdo_enabled && self.backend != Backend::Demo && !active {
            ui.checkbox(
                &mut self.bus_confirmed,
                "确认总线已停机且无其他主站。PDO 模式独占主站，进入 OP 并持续发送零值输出。",
            );
        } else if self.backend == Backend::Soem && !active {
            ui.checkbox(
                &mut self.bus_confirmed,
                "确认总线已停机且无其他主站。SOEM 扫描会将从站切到 INIT。",
            );
        }
        if self.backend == Backend::Igh && !cfg!(target_os = "linux") {
            ui.colored_label(WARN, "IgH 后端需要 Linux 主站。");
        }
    }

    fn overview(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.colored_label(FAULT, error);
        }
        if self.stopping {
            ui.label("正在断开…");
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            ui.weak(if self.session.is_some() {
                "等待主站响应…"
            } else {
                "未连接"
            });
            return;
        };
        let (status, color) = match snapshot.status.as_str() {
            "up" => ("链路已连接", GOOD),
            "down" => ("链路断开", FAULT),
            "scanning" => ("IgH 正在扫描", WARN),
            "responding" => ("总线有响应", GOOD),
            "silent" => ("未发现从站响应", WARN),
            "changed" => ("响应数量曾变化，请检查接线并重新连接", WARN),
            "demo" => ("演示数据", GOOD),
            _ => ("状态未知", WARN),
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color, status);
            ui.separator();
            ui.label(format!("报告从站数 {}", snapshot.count));
            ui.separator();
            ui.label(format!("显示 {} 台", snapshot.slaves.len()));
            ui.separator();
            ui.weak(format!(
                "上次采样 {:.1} s 前",
                snapshot.captured.elapsed().as_secs_f32()
            ));
        });
    }
}

fn al_state(raw: Option<u16>) -> (String, Color32) {
    let Some(raw) = raw else {
        return ("无响应".into(), FAULT);
    };
    let (label, color) = match raw & 0x0f {
        0x01 => ("INIT", WARN),
        0x02 => ("PRE-OP", WARN),
        0x03 => ("BOOT", WARN),
        0x04 => ("SAFE-OP", WARN),
        0x08 => ("OP", GOOD),
        _ => ("未知", WARN),
    };
    if raw & 0x10 != 0 {
        (format!("{label} + ERR"), FAULT)
    } else {
        (label.into(), color)
    }
}

impl eframe::App for Monitor {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.receive_updates();
        if self.session.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.workbench(ctx);
    }
}

fn configure_font(ctx: &egui::Context) {
    let custom = std::env::var_os("ECAT_MONITOR_FONT").and_then(|path| fs::read(path).ok());
    let system = [
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simsun.ttc",
    ];
    if let Some(font) = custom.or_else(|| system.iter().find_map(|path| fs::read(path).ok())) {
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".into(), egui::FontData::from_owned(font).into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("cjk".into());
        }
        ctx.set_fonts(fonts);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([720.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EtherCAT Monitor",
        options,
        Box::new(|creation| {
            creation.egui_ctx.set_theme(egui::Theme::Dark);
            workbench::configure_theme(&creation.egui_ctx);
            configure_font(&creation.egui_ctx);
            let mut monitor = Monitor::default();
            if std::env::args().any(|argument| argument == "--demo") {
                monitor.advanced.demo(&creation.egui_ctx);
            }
            Ok(Box::new(monitor))
        }),
    )
}
