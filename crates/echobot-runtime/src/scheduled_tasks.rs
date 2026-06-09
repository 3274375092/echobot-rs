//! Scheduled-task executors: closures that the cron / heartbeat services
//! invoke when jobs fire.
//!
//! Mirrors `echobot/runtime/scheduled_tasks.py`. The executor factories take
//! a [`SessionAgentRunner`] and a coordinator placeholder (anything that
//! implements the local [`CoordinatorLike`] trait) and return a closure
//! the scheduler stores as `on_job` / `on_execute`.
//!
//! ## Coordinator placeholder
//!
//! The Python version passes in a `ConversationCoordinator` (from the
//! orchestration crate) which is responsible for "presenting" scheduled
//! notifications. Since orchestration is not yet ported, we accept any
//! `T: CoordinatorLike` and the orchestration agent will implement this
//! trait in the next slice.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use echobot_core::models::message_content_to_text;

use crate::error::Result;
use crate::scheduling::cron::{CronJob, CronPayloadKind};
use crate::session_runner::{SessionAgentRunner, SessionRunResult};
use crate::turns::TraceCallback;

/// Signature of the scheduled-notification notifier. The four arguments are
/// `(session_name, source, job_name, visible_content)`.
pub type ScheduleNotifier = Arc<
    dyn Fn(
            String,
            String,
            String,
            String,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Trait the conversation coordinator must implement. The orchestration
/// crate's `ConversationCoordinator` will `impl CoordinatorLike` in the
/// orchestration slice.
pub trait CoordinatorLike: Send + Sync {
    /// Renders a scheduled notification for the given session and returns
    /// the visible text to forward to the user.
    fn present_scheduled_notification<'a>(
        &'a self,
        session_name: &'a str,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;
}

/// No-op coordinator used by tests / examples.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullCoordinator;

impl CoordinatorLike for NullCoordinator {
    fn present_scheduled_notification<'a>(
        &'a self,
        _session_name: &'a str,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        let text = content.to_string();
        Box::pin(async move { Ok(text) })
    }
}

/// Builds the cron-job executor closure. The returned `JobExecutor` matches
/// the signature used by [`crate::scheduling::cron::CronService`].
pub fn build_cron_job_executor<C: CoordinatorLike + 'static>(
    session_runner: Arc<SessionAgentRunner>,
    coordinator: Arc<C>,
    notify: ScheduleNotifier,
) -> impl Fn(
    CronJob,
) -> Pin<
    Box<dyn Future<Output = Result<Option<String>>> + Send>,
> + Send
+ Sync {
    move |job: CronJob| {
        let session_runner = session_runner.clone();
        let coordinator = coordinator.clone();
        let notify = notify.clone();
        Box::pin(async move {
            match job.payload.kind {
                CronPayloadKind::Text => {
                    session_runner
                        .append_assistant_message(&job.payload.session_name, &job.payload.content)
                        .await?;
                    let visible = coordinator
                        .present_scheduled_notification(
                            &job.payload.session_name,
                            &job.payload.content,
                        )
                        .await?;
                    notify(
                        job.payload.session_name.clone(),
                        "cron".to_string(),
                        job.name.clone(),
                        visible.clone(),
                    )
                    .await?;
                    Ok(Some(visible))
                }
                CronPayloadKind::Agent => {
                    let execution: SessionRunResult = session_runner
                        .run_prompt(
                            &job.payload.session_name,
                            &job.payload.content,
                            None,
                            None,
                            true,
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await?;
                    let raw =
                        message_content_to_text(&execution.agent_result.response.message.content)
                            .trim()
                            .to_string();
                    if raw.is_empty() {
                        return Ok(None);
                    }
                    let visible = coordinator
                        .present_scheduled_notification(&job.payload.session_name, &raw)
                        .await?;
                    notify(
                        job.payload.session_name.clone(),
                        "cron".to_string(),
                        job.name.clone(),
                        visible.clone(),
                    )
                    .await?;
                    Ok(Some(visible))
                }
            }
        })
    }
}

/// Builds the heartbeat executor closure. The returned `HeartbeatExecutor`
/// matches the signature used by [`crate::scheduling::heartbeat::HeartbeatService`].
pub fn build_heartbeat_executor(
    session_runner: Arc<SessionAgentRunner>,
) -> impl Fn(String) -> Pin<Box<dyn Future<Output = Result<Option<String>>> + Send>> + Send + Sync {
    move |tasks: String| {
        let session_runner = session_runner.clone();
        Box::pin(async move {
            let execution: SessionRunResult = session_runner
                .run_prompt(
                    "heartbeat",
                    &tasks,
                    None,
                    None,
                    true,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            let raw = message_content_to_text(&execution.agent_result.response.message.content)
                .trim()
                .to_string();
            Ok(if raw.is_empty() { None } else { Some(raw) })
        })
    }
}

// Keep the unused-import warning quiet for `TraceCallback`; downstream
// callers can use it without the linter complaining.
#[allow(dead_code)]
fn _ensure_trace_callback_in_scope(_cb: Option<TraceCallback>) {}
