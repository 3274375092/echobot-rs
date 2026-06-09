//! Adapters between the runtime crate's traits and the concrete types in
//! the tools / orchestration crates.
//!
//! The runtime crate's [`ToolRegistryLike`] and [`SkillRegistryLike`]
//! traits are intentionally minimal so the runtime stays orchestration-
//! free. The CLI's bridge module wraps the concrete
//! [`echobot_tools::ToolRegistry`] and
//! [`echobot_skill::SkillRegistry`] types in newtypes that implement
//! those traits, and adapts the runtime's [`CronService`] to the trait
//! consumed by [`echobot_tools::CronTool`].

use std::sync::Arc;

use async_trait::async_trait;

use echobot_core::models::LLMMessage;
use echobot_runtime::scheduling::cron::CronService as RuntimeCronService;
use echobot_runtime::turns::{SkillRegistryLike, ToolRegistryLike};
use echobot_skill::SkillRegistry;
use echobot_tools::cron::{CronJob, CronJobSummary, CronPayload as ToolCronPayload, CronSchedule, CronService as ToolCronService};
use echobot_tools::ToolRegistry;

/// Adapter that implements the runtime's [`ToolRegistryLike`] for the
/// concrete [`ToolRegistry`].
pub struct RuntimeToolAdapter(pub ToolRegistry);

#[async_trait]
impl ToolRegistryLike for RuntimeToolAdapter {
    fn names(&self) -> Vec<String> {
        self.0.names()
    }
}

/// Adapter that implements the runtime's [`SkillRegistryLike`] for the
/// concrete [`SkillRegistry`].
pub struct RuntimeSkillAdapter(pub Arc<SkillRegistry>);

#[async_trait]
impl SkillRegistryLike for RuntimeSkillAdapter {
    fn active_skill_names(&self, history: Option<&[LLMMessage]>) -> Vec<String> {
        history
            .map(|h| self.0.active_skill_names_from_history(h))
            .unwrap_or_default()
    }

    fn explicit_skill_names(&self, user_input: &str) -> Vec<String> {
        self.0.explicit_skill_names(user_input)
    }
}

/// Bridge from the runtime's [`RuntimeCronService`] (the concrete
/// scheduler) to the tools' `CronService` trait consumed by
/// [`echobot_tools::CronTool`].
pub struct RuntimeCronAdapter {
    inner: Arc<RuntimeCronService>,
}

impl RuntimeCronAdapter {
    /// Wraps the runtime service.
    pub fn new(inner: Arc<RuntimeCronService>) -> Self {
        Self { inner }
    }

    /// Returns a clone of the inner service (for the cron tool or
    /// external callers).
    pub fn inner(&self) -> Arc<RuntimeCronService> {
        self.inner.clone()
    }
}

#[async_trait]
impl ToolCronService for RuntimeCronAdapter {
    async fn add_job(
        &self,
        name: &str,
        schedule: &CronSchedule,
        payload: &ToolCronPayload,
        delete_after_run: bool,
    ) -> Result<CronJob, echobot_core::Error> {
        let rt_schedule = schedule_kind_from_tool(schedule)?;
        let rt_payload = echobot_runtime::scheduling::cron::CronPayload {
            kind: payload_kind_from_tool(&payload.kind)?,
            content: payload.content.clone(),
            session_name: payload.session_name.clone(),
        };
        let job = self
            .inner
            .add_job(name, rt_schedule, rt_payload, delete_after_run)
            .await
            .map_err(core_from_runtime)?;
        Ok(CronJob {
            summary: summarize_job(&job),
        })
    }

    async fn list_jobs(&self, include_disabled: bool) -> Result<Vec<CronJob>, echobot_core::Error> {
        let jobs = self.inner.list_jobs(include_disabled).await;
        Ok(jobs
            .into_iter()
            .map(|j| CronJob { summary: summarize_job(&j) })
            .collect())
    }

    async fn remove_job(&self, job_id: &str) -> Result<bool, echobot_core::Error> {
        self.inner
            .remove_job(job_id)
            .await
            .map_err(core_from_runtime)
    }

    async fn run_job(&self, job_id: &str, force: bool) -> Result<bool, echobot_core::Error> {
        self.inner
            .run_job(job_id, force)
            .await
            .map_err(core_from_runtime)
    }

    async fn set_enabled(
        &self,
        job_id: &str,
        enabled: bool,
    ) -> Result<Option<CronJob>, echobot_core::Error> {
        let updated = self
            .inner
            .set_enabled(job_id, enabled)
            .await
            .map_err(core_from_runtime)?;
        Ok(updated.map(|j| CronJob { summary: summarize_job(&j) }))
    }
}

fn schedule_kind_from_tool(
    schedule: &CronSchedule,
) -> Result<echobot_runtime::scheduling::cron::CronSchedule, echobot_core::Error> {
    use echobot_runtime::scheduling::cron::{CronSchedule as Rt, CronScheduleKind};
    let kind = match schedule.kind.as_str() {
        "at" => CronScheduleKind::At,
        "every" => CronScheduleKind::Every,
        "cron" => CronScheduleKind::Cron,
        other => {
            return Err(echobot_core::Error::Tool(
                echobot_core::ToolError::InvalidValue {
                    name: "schedule.kind".to_string(),
                    message: format!("unknown cron kind from tool: {other}"),
                },
            ));
        }
    };
    let value = schedule.value.trim();
    Ok(match kind {
        CronScheduleKind::At => Rt {
            kind,
            at: Some(value.to_string()),
            every_seconds: None,
            expr: None,
            timezone: schedule.timezone.clone(),
        },
        CronScheduleKind::Every => {
            let secs: u64 = value.parse().map_err(|_| {
                echobot_core::Error::Tool(echobot_core::ToolError::InvalidValue {
                    name: "schedule.value".to_string(),
                    message: format!("every schedule needs an integer: {value}"),
                })
            })?;
            Rt {
                kind,
                at: None,
                every_seconds: Some(secs),
                expr: None,
                timezone: None,
            }
        }
        CronScheduleKind::Cron => Rt {
            kind,
            at: None,
            every_seconds: None,
            expr: Some(value.to_string()),
            timezone: schedule.timezone.clone(),
        },
    })
}

fn payload_kind_from_tool(
    kind: &str,
) -> Result<echobot_runtime::scheduling::cron::CronPayloadKind, echobot_core::Error> {
    use echobot_runtime::scheduling::cron::CronPayloadKind;
    match kind {
        "agent" => Ok(CronPayloadKind::Agent),
        "text" => Ok(CronPayloadKind::Text),
        other => Err(echobot_core::Error::Tool(
            echobot_core::ToolError::InvalidValue {
                name: "kind".to_string(),
                message: format!("unknown cron payload kind from tool: {other}"),
            },
        )),
    }
}

fn summarize_job(job: &echobot_runtime::scheduling::cron::CronJob) -> CronJobSummary {
    use echobot_runtime::scheduling::cron::{CronPayloadKind, CronScheduleKind};
    let (kind, value) = match job.schedule.kind {
        CronScheduleKind::At => (
            "at".to_string(),
            job.schedule.at.clone().unwrap_or_default(),
        ),
        CronScheduleKind::Every => (
            "every".to_string(),
            job.schedule
                .every_seconds
                .map(|n| n.to_string())
                .unwrap_or_default(),
        ),
        CronScheduleKind::Cron => (
            "cron".to_string(),
            job.schedule.expr.clone().unwrap_or_default(),
        ),
    };
    CronJobSummary {
        id: job.id.clone(),
        name: job.name.clone(),
        schedule_kind: kind,
        schedule_value: value,
        timezone: job.schedule.timezone.clone(),
        task_kind: match job.payload.kind {
            CronPayloadKind::Agent => "agent".to_string(),
            CronPayloadKind::Text => "text".to_string(),
        },
        content: job.payload.content.clone(),
        session_name: job.payload.session_name.clone(),
        enabled: job.enabled,
        delete_after_run: job.delete_after_run,
    }
}

fn core_from_runtime(err: echobot_runtime::error::Error) -> echobot_core::Error {
    echobot_core::Error::Config(echobot_core::error::ConfigError::InvalidEnvValue {
        name: "runtime".to_string(),
        message: err.to_string(),
    })
}
