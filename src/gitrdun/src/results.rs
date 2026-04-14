use anyhow::Result;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::cli::Args;
use crate::git::SearchResult;
use crate::ui::ProgressDisplay;

/// Format search results into a string buffer.
///
/// This is a pure function: it returns the formatted output without any
/// file-system side effects. The caller is responsible for writing the
/// returned string to a file and/or stdout.
///
/// # Errors
///
/// Returns an error if Ollama summary generation fails and cannot be
/// recovered from.
pub async fn format_results(
    _result: &SearchResult,
    _args: &Args,
    _use_ollama: bool,
    _progress: Option<Arc<ProgressDisplay>>,
    _cancellation_token: CancellationToken,
) -> Result<String> {
    // Stub - real implementation comes in the Green step.
    Ok(String::new())
}
