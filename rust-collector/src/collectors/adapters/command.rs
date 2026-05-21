//! Command adapter for external collectors.

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;

pub struct CommandPlugin {
    pub name: String,
    pub command: String,
    pub timeout: Duration,
}

impl CollectorPlugin for CommandPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn collect(&mut self, _now: DateTime<Utc>, _config: &CollectorConfig) -> Result<PluginOutput> {
        let mut command = Command::new("sh");
        command
            .arg("-lc")
            .arg(&self.command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let started = Instant::now();

        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if started.elapsed() >= self.timeout {
                kill_child_tree(&mut child);
                let _ = child.wait();
                return Err(CollectorError::Plugin {
                    plugin: self.name.clone(),
                    message: format!("command timed out after {} ms", self.timeout.as_millis()),
                });
            }
            thread::sleep(Duration::from_millis(25));
        }

        let output = child.wait_with_output()?;

        if !output.status.success() {
            return Err(CollectorError::Plugin {
                plugin: self.name.clone(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

fn kill_child_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill").args(["-TERM", &group]).status();
        thread::sleep(Duration::from_millis(50));
        let _ = Command::new("kill").args(["-KILL", &group]).status();
        return;
    }

    #[allow(unreachable_code)]
    {
        let _ = child.kill();
    }
}
