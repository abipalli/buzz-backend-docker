//! The substrate: a Docker host reached through the `docker` CLI and its
//! ambient context (spec I2 — credentials come from `docker context`, never
//! from `provider_config`).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    pub id: String,
    pub status: String,
    pub labels: BTreeMap<String, String>,
    pub exit_code: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    pub name: String,
    pub image: String,
    pub labels: BTreeMap<String, String>,
    pub network: Option<String>,
    pub add_hosts: Vec<String>,
    pub mounts: Vec<String>,
    pub cpus: String,
    pub memory: String,
    pub stop_timeout_secs: u32,
}

#[derive(Debug, PartialEq)]
pub enum RunError {
    /// Another deploy owns the deterministic name: re-read and adopt.
    Conflict,
    Failed(String),
}

#[derive(Debug, PartialEq)]
pub enum RemoveOutcome {
    Removed,
    /// Refused because the container is running — the fence that makes
    /// classify-then-delete safe without force.
    StillRunning,
}

pub trait Substrate {
    fn inspect(&self, name: &str) -> Result<Option<Observed>, String>;
    fn run(&self, spec: &RunSpec, env: &BTreeMap<String, String>) -> Result<String, RunError>;
    fn remove(&self, id: &str) -> Result<RemoveOutcome, String>;
    fn describe(&self) -> String;
    /// Whether `command` resolves on the image's PATH, checked before a
    /// container is created so a missing runtime fails Start immediately.
    fn image_has_command(&self, image: &str, command: &str) -> Result<bool, String>;
}

/// Environment the docker CLI itself reads. An agent variable with one of
/// these names would reconfigure the CLI instead of reaching the container.
const CLI_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "TMPDIR",
    "LANG",
    "SSH_AUTH_SOCK",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
];

pub fn cli_reserved(key: &str) -> bool {
    key.starts_with("DOCKER_") || CLI_ENV.contains(&key)
}

pub struct DockerCli {
    pub host: Option<String>,
    pub context: Option<String>,
}

impl DockerCli {
    fn command(&self) -> Command {
        let mut cmd = Command::new("docker");
        cmd.env_clear();
        for (k, v) in std::env::vars() {
            if cli_reserved(&k) {
                cmd.env(k, v);
            }
        }
        cmd.env("PATH", augmented_path());
        if let Some(host) = &self.host {
            cmd.args(["-H", host]);
        } else if let Some(ctx) = &self.context {
            cmd.args(["--context", ctx]);
        }
        cmd.stdin(Stdio::null());
        cmd
    }

    /// Create `name` if it does not exist. Never modifies or removes an
    /// existing network.
    pub fn ensure_network(&self, name: &str) -> Result<(), String> {
        let out = self
            .command()
            .args(["network", "inspect", "--format", "{{.Name}}", name])
            .output()
            .map_err(|e| format!("could not run docker: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let err = stderr_text(&out);
        if !err.to_ascii_lowercase().contains("not found")
            && !err.to_ascii_lowercase().contains("no such")
        {
            return Err(format!("docker network inspect failed: {err}"));
        }
        let label = format!(
            "{}={}",
            crate::naming::LABEL_MANAGED_BY,
            crate::naming::MANAGED_BY
        );
        let out = self
            .command()
            .args(["network", "create", "--label", &label, name])
            .output()
            .map_err(|e| format!("could not run docker: {e}"))?;
        if out.status.success() || stderr_text(&out).contains("already exists") {
            return Ok(());
        }
        Err(format!(
            "could not create network {name}: {}",
            stderr_text(&out)
        ))
    }

    /// Server version, as a reachability check for `setup`.
    pub fn server_version(&self) -> Result<String, String> {
        let out = self
            .command()
            .args(["version", "--format", "{{.Server.Version}}"])
            .output()
            .map_err(|e| format!("could not run docker: {e} (install Docker Desktop, or just the CLI with: brew install docker)"))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
        Err(stderr_text(&out))
    }
}

/// A GUI-launched desktop hands providers launchd's minimal PATH; the docker
/// CLI and its ssh/credential helpers usually live in these directories.
fn augmented_path() -> String {
    let mut dirs = vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(format!("{home}/.local/bin"));
    }
    if let Ok(path) = std::env::var("PATH") {
        dirs.push(path);
    }
    dirs.join(":")
}

#[derive(Deserialize)]
struct InspectState {
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "ExitCode", default)]
    exit_code: i64,
}

#[derive(Deserialize)]
struct InspectConfig {
    #[serde(rename = "Labels", default)]
    labels: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct Inspect {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "State")]
    state: InspectState,
    #[serde(rename = "Config")]
    config: InspectConfig,
}

fn stderr_text(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .trim()
        .chars()
        .take(2000)
        .collect()
}

impl Substrate for DockerCli {
    fn inspect(&self, name: &str) -> Result<Option<Observed>, String> {
        let out = self
            .command()
            .args(["container", "inspect", name])
            .output()
            .map_err(|e| {
                format!("could not run docker: {e} (is the docker CLI installed and on PATH?)")
            })?;
        if !out.status.success() {
            let err = stderr_text(&out);
            if err.to_ascii_lowercase().contains("no such") {
                return Ok(None);
            }
            return Err(format!("docker inspect failed: {err}"));
        }
        let parsed: Vec<Inspect> = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("could not parse docker inspect output: {e}"))?;
        Ok(parsed.into_iter().next().map(|c| Observed {
            id: c.id,
            status: c.state.status,
            labels: c.config.labels.unwrap_or_default(),
            exit_code: c.state.exit_code,
        }))
    }

    fn run(&self, spec: &RunSpec, env: &BTreeMap<String, String>) -> Result<String, RunError> {
        let mut cmd = self.command();
        cmd.args([
            "run",
            "--detach",
            "--pull",
            "missing",
            "--name",
            &spec.name,
            "--hostname",
            &spec.name,
        ]);
        cmd.args([
            "--restart",
            "no",
            "--stop-timeout",
            &spec.stop_timeout_secs.to_string(),
        ]);
        cmd.args(["--cap-drop", "ALL", "--security-opt", "no-new-privileges"]);
        cmd.args(["--cpus", &spec.cpus, "--memory", &spec.memory]);
        if let Some(net) = &spec.network {
            cmd.args(["--network", net]);
        }
        for host in &spec.add_hosts {
            cmd.args(["--add-host", host]);
        }
        for mount in &spec.mounts {
            cmd.args(["--mount", mount]);
        }
        for (k, v) in &spec.labels {
            cmd.args(["--label", &format!("{k}={v}")]);
        }
        for (k, v) in env {
            cmd.env(k, v);
            cmd.args(["--env", k]);
        }
        cmd.arg(&spec.image);
        let out = cmd
            .output()
            .map_err(|e| RunError::Failed(format!("could not run docker: {e}")))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
        let err = stderr_text(&out);
        if err.contains("is already in use") || err.contains("Conflict.") {
            return Err(RunError::Conflict);
        }
        Err(RunError::Failed(format!("docker run failed: {err}")))
    }

    fn remove(&self, id: &str) -> Result<RemoveOutcome, String> {
        let out = self
            .command()
            .args(["container", "rm", id])
            .output()
            .map_err(|e| format!("could not run docker: {e}"))?;
        if out.status.success() {
            return Ok(RemoveOutcome::Removed);
        }
        let err = stderr_text(&out);
        let lower = err.to_ascii_lowercase();
        if lower.contains("no such") {
            return Ok(RemoveOutcome::Removed);
        }
        if lower.contains("running") {
            return Ok(RemoveOutcome::StillRunning);
        }
        Err(format!("docker rm failed: {err}"))
    }

    fn image_has_command(&self, image: &str, command: &str) -> Result<bool, String> {
        let out = self
            .command()
            .args([
                "run",
                "--rm",
                "--pull",
                "missing",
                "--network",
                "none",
                "--cap-drop",
                "ALL",
            ])
            .args([
                "--entrypoint",
                "sh",
                image,
                "-c",
                "command -v \"$1\" >/dev/null",
                "sh",
                command,
            ])
            .output()
            .map_err(|e| format!("could not run docker: {e}"))?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) | Some(127) => Ok(false),
            _ => Err(format!(
                "could not inspect the agent image {image}: {}",
                stderr_text(&out)
            )),
        }
    }

    fn describe(&self) -> String {
        match (&self.host, &self.context) {
            (Some(host), _) => format!("docker -H {host}"),
            (None, Some(ctx)) => format!("docker --context {ctx}"),
            (None, None) => "docker".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_reserved_covers_docker_and_shell_env() {
        for k in [
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "PATH",
            "HOME",
            "SSH_AUTH_SOCK",
        ] {
            assert!(cli_reserved(k), "{k}");
        }
        for k in ["BUZZ_PRIVATE_KEY", "OPENAI_API_KEY", "GOOSE_MODEL"] {
            assert!(!cli_reserved(k), "{k}");
        }
    }
}
