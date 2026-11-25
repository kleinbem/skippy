//! `skippy analytics`, and what each `skippy` command reports.
//!
//! The event vocabulary, consent rules and delivery live in
//! `skippy-analytics`; this module decides when the CLI calls them.

use std::time::Instant;

use anyhow::Result;
use skippy_analytics::{ConsentInputs, Event, Properties};
use skippy_commands::console;

use crate::cli::{AnalyticsCommand, Command};

/// Start standalone reporting under the recorded preference.
///
/// `skippy analytics` itself never starts it: asking what is collected, or
/// turning it off, must not report anything or create an identifier.
pub fn init(command: &Command) {
    if matches!(
        command,
        Command::Analytics { .. } | Command::TuneTrial { .. }
    ) {
        return;
    }
    let preference = skippy_analytics::state_dir()
        .map(|dir| skippy_analytics::read_preference(&dir))
        .unwrap_or_default();
    skippy_analytics::init(preference);
}

/// The family a command reports under. Arguments never go out.
pub fn family(command: &Command) -> &'static str {
    match command {
        Command::Doctor { .. } => "doctor",
        Command::GpuBenchmark { .. } => "gpu_benchmark",
        Command::Prompt(_) => "prompt",
        Command::Serve(_) => "serve",
        Command::Tune(_) => "tune",
        Command::TuneTrial { .. } => "tune_trial",
        #[cfg(unix)]
        Command::ServeManaged(_) => "serve_managed",
        #[cfg(unix)]
        Command::Manage(_) => "manage",
        Command::ExampleConfig => "example_config",
        Command::Models { .. } => "models",
        Command::PlanSplit(_) => "plan_split",
        Command::Runtime { .. } => "runtime",
        Command::Analytics { .. } => "analytics",
    }
}

/// Report a finished one-shot command by family and outcome.
pub fn record_command(family: &'static str, succeeded: bool) {
    skippy_analytics::capture(
        Event::CliCommand,
        Properties::new()
            .with("family", family)
            .with("outcome", if succeeded { "completed" } else { "failed" }),
    );
}

/// A `skippy serve` session, measured from start to shutdown.
pub struct ServeSession {
    started: Instant,
}

impl ServeSession {
    /// Report the start, the machine, and begin reporting generations.
    pub fn start(stage_worker: bool) -> Self {
        skippy_analytics::generation::install();
        skippy_analytics::host_profile::report();
        skippy_analytics::capture(
            Event::ServeStarted,
            Properties::new().with("surface", if stage_worker { "stage" } else { "public" }),
        );
        Self {
            started: Instant::now(),
        }
    }

    /// Report the end, with a bucketed session length.
    pub fn finish(self, succeeded: bool) {
        skippy_analytics::capture(
            Event::ServeStopped,
            Properties::new()
                .with(
                    "session_length",
                    skippy_analytics::bucket_duration_secs(self.started.elapsed().as_secs()),
                )
                .with("succeeded", succeeded),
        );
    }
}

/// `skippy analytics status|enable|disable`.
pub fn run(command: AnalyticsCommand) -> Result<()> {
    let dir = skippy_analytics::state_dir()?;
    match command {
        AnalyticsCommand::Enable => {
            skippy_analytics::write_preference(&dir, true)?;
            console::status("✅ Anonymous usage analytics enabled")?;
        }
        AnalyticsCommand::Disable => {
            skippy_analytics::write_preference(&dir, false)?;
            console::status("✅ Anonymous usage analytics disabled")?;
        }
        AnalyticsCommand::Status => {}
    }
    let disposition = ConsentInputs::from_env(skippy_analytics::read_preference(&dir)).resolve();
    let install_id = skippy_analytics::load(&dir);
    let report = serde_json::json!({
        "enabled": disposition.is_enabled(),
        "reason": disposition.explain(),
        "install_id": install_id.as_ref().map(skippy_analytics::InstallId::as_str),
        "state_dir": dir,
        "endpoint": disposition.is_enabled().then(skippy_analytics::ingestion_host),
    });
    console::present(&report, |out| {
        writeln!(out, "Analytics: {}", disposition.explain())?;
        match &install_id {
            Some(id) => writeln!(out, "Install ID: {}", id.as_str())?,
            None => writeln!(out, "Install ID: none yet")?,
        }
        writeln!(out, "State: {}", dir.display())?;
        writeln!(out, "\n{}", skippy_analytics::NOTICE)
    })
}
