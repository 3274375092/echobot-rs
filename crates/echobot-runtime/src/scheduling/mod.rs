//! Scheduling primitives: cron + heartbeat.
//!
//! Mirrors `echobot/scheduling/*`. The submodules are public so callers can
//! pick whichever subset they need; the rest of the runtime can re-export
//! them as `pub use scheduling::*;` if it wants a flat surface.

pub mod cron;
pub mod heartbeat;

pub use cron::{
    compute_next_run, describe_schedule, normalize_schedule, summarize_job, CronJob,
    CronJobState, CronPayload, CronPayloadKind, CronSchedule, CronScheduleKind, CronService,
    CronStore, JobExecutor, StatusReport,
};
pub use heartbeat::{
    has_meaningful_heartbeat_content, heartbeat_decision_tool, read_or_create_heartbeat_file,
    write_heartbeat_file, DEFAULT_HEARTBEAT_TEMPLATE, HeartbeatExecutor, HeartbeatNotifier,
    HeartbeatService,
};
