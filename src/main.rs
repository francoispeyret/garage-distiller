#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

fn main() -> eframe::Result {
    if std::env::args().any(|arg| arg == "--version") {
        println!("garage-distiller {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let _ = notify_rust::set_application("fr.francoispeyret.garage-distiller");
    let icon = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .expect("embedded application icon")
        .into_rgba8();
    let icon = eframe::egui::IconData {
        width: icon.width(),
        height: icon.height(),
        rgba: icon.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1380.0, 900.0])
            .with_min_inner_size([1050.0, 700.0])
            .with_app_id("fr.francoispeyret.garage-distiller")
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "Garage Distiller",
        options,
        Box::new(|cc| Ok(Box::new(ui::GarageApp::new(cc)))),
    )
}
