use anyhow::Result;
use crate::ExportFormat;

pub mod web;
pub mod video;
pub mod terminal_renderer;

pub async fn handle_export(format: ExportFormat) -> Result<()> {
    match format {
        ExportFormat::Web { input, output, theme, compress } => {
            web::export_web(input, output, theme, compress).await
        }
        ExportFormat::Video { input, output, fps, resolution, theme, optimize_web } => {
            video::export_video(input, output, fps, resolution, theme, optimize_web).await
        }
    }
}
