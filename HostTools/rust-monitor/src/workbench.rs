use crate::{advanced::Advanced, trace::Watch, trace_view, GOOD};
use eframe::egui::{self, Color32, Frame, Margin, RichText, Stroke};
use ethercat_monitor::backend::Backend;
use std::{env, fs, path::PathBuf};

const CANVAS: Color32 = Color32::from_rgb(13, 20, 29);
const SURFACE: Color32 = Color32::from_rgb(21, 31, 43);
const BORDER: Color32 = Color32::from_rgb(48, 65, 81);
const TEXT: Color32 = Color32::from_rgb(229, 238, 247);
const SELECTED: Color32 = Color32::from_rgb(23, 67, 77);

pub(crate) fn configure_theme(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.spacing.item_spacing = egui::vec2(9.0, 8.0);
        style.spacing.interact_size.y = 28.0;
        style.visuals.panel_fill = SURFACE;
        style.visuals.window_fill = SURFACE;
        style.visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
        style.visuals.extreme_bg_color = CANVAS;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = SELECTED;
        style.visuals.selection.stroke = Stroke::new(1.0_f32, GOOD);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(34, 48, 62);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 64, 81);
        style.visuals.widgets.active.bg_fill = SELECTED;
        for text_style in [
            egui::TextStyle::Body,
            egui::TextStyle::Button,
            egui::TextStyle::Monospace,
        ] {
            if let Some(font) = style.text_styles.get_mut(&text_style) {
                font.size = 14.0;
            }
        }
    });
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Profile {
    backend: String,
    target: String,
    cycle_us: u32,
    dc: bool,
    activation: String,
    watches: Vec<Watch>,
    left: f32,
    right: f32,
    bottom: f32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            backend: "demo".into(),
            target: String::new(),
            cycle_us: 5000,
            dc: false,
            activation: "0x0300".into(),
            watches: Vec::new(),
            left: 0.16,
            right: 0.24,
            bottom: 0.40,
        }
    }
}

fn profile_path() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var_os("APPDATA").map(PathBuf::from)
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }
    .map(|dir| dir.join("ethercat-monitor").join("workbench-v1.json"))
}

pub struct Workbench {
    app: Advanced,
    timeline: trace_view::Timeline,
    path: Option<PathBuf>,
    panels: [bool; 3],
    proportions: [f32; 3],
}

impl Workbench {
    pub fn new(ctx: &egui::Context, demo: bool) -> Self {
        let path = if demo { None } else { profile_path() };
        let profile: Profile = path
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        let mut app = Advanced::default();
        app.backend = match profile.backend.as_str() {
            "soem" => Backend::Soem,
            "igh" if cfg!(target_os = "linux") => Backend::Igh,
            _ => Backend::Demo,
        };
        app.target = profile.target;
        app.cycle_us = profile.cycle_us.clamp(100, 20_000);
        app.dc = profile.dc;
        app.activation = profile.activation;
        app.watches = profile.watches.into_iter().take(256).collect();
        if demo {
            app.demo(ctx);
        }
        Self {
            app,
            timeline: trace_view::Timeline::default(),
            path,
            panels: [true; 3],
            proportions: [
                profile.left.clamp(0.12, 0.33),
                profile.right.clamp(0.15, 0.36),
                profile.bottom.clamp(0.20, 0.65),
            ],
        }
    }

    fn save_profile(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let profile = Profile {
            backend: match self.app.backend {
                Backend::Igh => "igh",
                Backend::Soem => "soem",
                Backend::Demo => "demo",
            }
            .into(),
            target: self.app.target.clone(),
            cycle_us: self.app.cycle_us,
            dc: self.app.dc,
            activation: self.app.activation.clone(),
            watches: self.app.watches.clone(),
            left: self.proportions[0],
            right: self.proportions[1],
            bottom: self.proportions[2],
        };
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        fs::create_dir_all(parent)?;
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temp, serde_json::to_vec_pretty(&profile)?)?;
        fs::rename(temp, path)
    }
}

impl eframe::App for Workbench {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let size = ctx.screen_rect().size();
        let surface = |x, y| {
            Frame::new()
                .fill(SURFACE)
                .inner_margin(Margin::symmetric(x, y))
        };
        egui::TopBottomPanel::top("toolbar")
            .frame(surface(12, 6))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("EtherCAT 工作台").size(18.0).strong());
                    ui.separator();
                    for (open, label) in self.panels.iter_mut().zip(["从站树", "观察", "时间轴"])
                    {
                        if ui.selectable_label(*open, label).clicked() {
                            *open = !*open;
                        }
                    }
                });
                self.app.controls(ui);
            });
        egui::TopBottomPanel::bottom("bus_status")
            .frame(surface(12, 4))
            .show(ctx, |ui| {
                self.app.status(ui);
            });
        if self.panels[2] {
            let response = egui::TopBottomPanel::bottom("trace")
                .resizable(true)
                .height_range(155.0..=(size.y * 0.65).max(155.0))
                .default_height(size.y * self.proportions[2])
                .frame(surface(10, 4))
                .show(ctx, |ui| self.timeline.show(&mut self.app, ui));
            self.proportions[2] =
                (response.response.rect.height() / size.y.max(1.0)).clamp(0.20, 0.65);
        }
        if self.panels[0] {
            let shown = egui::SidePanel::left("topology")
                .resizable(true)
                .default_width(size.x * self.proportions[0])
                .width_range(143.0..=(size.x * 0.33).max(143.0))
                .frame(surface(10, 8))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("topology_rows")
                        .show(ui, |ui| self.app.sidebar(ui));
                });
            self.proportions[0] = (shown.response.rect.width() / size.x.max(1.0)).clamp(0.12, 0.33);
        }
        if self.panels[1] {
            let shown = egui::SidePanel::right("watch")
                .resizable(true)
                .default_width(size.x * self.proportions[1])
                .width_range(190.0..=(size.x * 0.36).max(190.0))
                .frame(surface(10, 8))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("watch_rows")
                        .show(ui, |ui| trace_view::observations(&mut self.app, ui));
                });
            self.proportions[1] = (shown.response.rect.width() / size.x.max(1.0)).clamp(0.15, 0.36);
        }
        egui::CentralPanel::default()
            .frame(Frame::new().fill(CANVAS).inner_margin(Margin::same(12)))
            .show(ctx, |ui| self.app.central(ui));
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        if let Err(error) = self.save_profile() {
            eprintln!("无法保存工作台设置：{error}");
        }
    }
}
