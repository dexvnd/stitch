#![windows_subsystem = "windows"]

mod app;
mod injector;
mod process;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 420.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_title("Stitch"),
        ..Default::default()
    };

    eframe::run_native(
        "stitch",
        options,
        Box::new(|cc| {
            app::apply_theme(&cc.egui_ctx);
            Ok(Box::new(app::StitchApp::new()))
        }),
    )
}
