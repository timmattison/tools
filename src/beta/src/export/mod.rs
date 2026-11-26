use crate::ExportFormat;
use anyhow::Result;

pub mod tmux;
pub mod video;
pub mod web;

pub async fn handle_export(format: ExportFormat) -> Result<()> {
    match format {
        ExportFormat::Web {
            input,
            output,
            theme,
            compress,
        } => web::export_web(input, output, theme, compress).await,
        ExportFormat::Video {
            input,
            output,
            fps,
            resolution,
            theme,
            optimize_web,
            font,
        } => video::export_video(input, output, fps, resolution, theme, optimize_web, font).await,
    }
}
