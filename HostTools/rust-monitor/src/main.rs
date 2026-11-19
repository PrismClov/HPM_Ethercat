#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod adapter_picker;
mod advanced;
mod trace;
mod trace_view;
mod workbench;

use eframe::egui::{self, Color32};
use std::fs;

const GOOD: Color32 = Color32::from_rgb(98, 222, 183);
const WARN: Color32 = Color32::from_rgb(246, 193, 100);
const FAULT: Color32 = Color32::from_rgb(252, 127, 128);

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
            let demo = std::env::args().any(|argument| argument == "--demo");
            Ok(Box::new(workbench::Workbench::new(
                &creation.egui_ctx,
                demo,
            )))
        }),
    )
}
