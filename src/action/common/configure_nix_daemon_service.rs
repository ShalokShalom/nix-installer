use std::path::{Path, PathBuf};

use target_lexicon::OperatingSystem;
use tokio::fs::remove_file;
use tokio::process::Command;
use tracing::{span, Span};

use crate::action::{ActionError, StatefulAction};
use crate::execute_command;

use crate::action::{Action, ActionDescription};
use crate::settings::InitSystem;

const SERVICE_SRC: &str = "/nix/var/nix/profiles/default/lib/systemd/system/nix-daemon.service";
const SOCKET_SRC: &str = "/nix/var/nix/profiles/default/lib/systemd/system/nix-daemon.socket";
const TMPFILES_SRC: &str = "/nix/var/nix/profiles/default/lib/tmpfiles.d/nix-daemon.conf";
const TMPFILES_DEST: &str = "/etc/tmpfiles.d/nix-daemon.conf";
const DARWIN_NIX_DAEMON_DEST: &str = "/Library/LaunchDaemons/org.nixos.nix-daemon.plist";
const DARWIN_NIX_DAEMON_SOURCE: &str =
    "/nix/var/nix/profiles/default/Library/LaunchDaemons/org.nixos.nix-daemon.plist";
/**
Configure the init to run the Nix daemon
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct ConfigureInitService {
    init: InitSystem,
}

impl ConfigureInitService {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(init: InitSystem) -> Result<StatefulAction<Self>, ActionError> {
        Ok(Self { init }.into())
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "configure_nix_daemon")]
impl Action for ConfigureInitService {
    fn tracing_synopsis(&self) -> String {
        match self.init {
            InitSystem::Systemd => "Configure Nix daemon related settings with systemd".to_string(),
            InitSystem::Launchd => {
                "Configure Nix daemon related settings with launchctl".to_string()
            },
            InitSystem::None => "Leave the Nix daemon unconfigured".to_string(),
        }
    }

    fn tracing_span(&self) -> Span {
        span!(tracing::Level::DEBUG, "configure_nix_daemon",)
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        match self.init {
            InitSystem::Systemd => {
                vec![ActionDescription::new(
                    self.tracing_synopsis(),
                    vec![
                        "Run `systemd-tempfiles --create --prefix=/nix/var/nix`".to_string(),
                        format!("Run `systemctl link {SERVICE_SRC}`"),
                        format!("Run `systemctl link {SOCKET_SRC}`"),
                        "Run `systemctl daemon-reload`".to_string(),
                    ],
                )]
            },
            InitSystem::Launchd => {
                vec![ActionDescription::new(
                    self.tracing_synopsis(),
                    vec![
                        format!("Copy `{DARWIN_NIX_DAEMON_SOURCE}` to `DARWIN_NIX_DAEMON_DEST`"),
                        format!("Run `launchctl load {DARWIN_NIX_DAEMON_DEST}`"),
                    ],
                )]
            },
            InitSystem::None => Vec::new(),
        }
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> Result<(), ActionError> {
        let Self { init } = self;

        match init {
            InitSystem::Launchd => {
                let src = Path::new(DARWIN_NIX_DAEMON_SOURCE);
                tokio::fs::copy(src.clone(), DARWIN_NIX_DAEMON_DEST)
                    .await
                    .map_err(|e| {
                        ActionError::Copy(
                            src.to_path_buf(),
                            PathBuf::from(DARWIN_NIX_DAEMON_DEST),
                            e,
                        )
                    })?;

                execute_command(
                    Command::new("launchctl")
                        .process_group(0)
                        .arg("load")
                        .arg(DARWIN_NIX_DAEMON_DEST)
                        .stdin(std::process::Stdio::null()),
                )
                .await
                .map_err(ActionError::Command)?;
            },
            InitSystem::Systemd => {
                tracing::trace!(src = TMPFILES_SRC, dest = TMPFILES_DEST, "Symlinking");
                tokio::fs::symlink(TMPFILES_SRC, TMPFILES_DEST)
                    .await
                    .map_err(|e| {
                        ActionError::Symlink(
                            PathBuf::from(TMPFILES_SRC),
                            PathBuf::from(TMPFILES_DEST),
                            e,
                        )
                    })?;

                execute_command(
                    Command::new("systemd-tmpfiles")
                        .process_group(0)
                        .arg("--create")
                        .arg("--prefix=/nix/var/nix")
                        .stdin(std::process::Stdio::null()),
                )
                .await
                .map_err(ActionError::Command)?;

                execute_command(
                    Command::new("systemctl")
                        .process_group(0)
                        .arg("link")
                        .arg(SERVICE_SRC)
                        .stdin(std::process::Stdio::null()),
                )
                .await
                .map_err(ActionError::Command)?;

                execute_command(
                    Command::new("systemctl")
                        .process_group(0)
                        .arg("link")
                        .arg(SOCKET_SRC)
                        .stdin(std::process::Stdio::null()),
                )
                .await
                .map_err(ActionError::Command)?;

                execute_command(
                    Command::new("systemctl")
                        .process_group(0)
                        .arg("daemon-reload")
                        .stdin(std::process::Stdio::null()),
                )
                .await
                .map_err(ActionError::Command)?;

                execute_command(
                    Command::new("systemctl")
                        .process_group(0)
                        .arg("enable")
                        .arg("--now")
                        .arg(SOCKET_SRC),
                )
                .await
                .map_err(ActionError::Command)?;
            },
            InitSystem::None => {
                // Nothing here, no init system
            },
        };

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        match self.init {
            InitSystem::Systemd => {
                vec![ActionDescription::new(
                    "Unconfigure Nix daemon related settings with systemd".to_string(),
                    vec![
                        "Run `systemctl disable {SOCKET_SRC}`".to_string(),
                        "Run `systemctl disable {SERVICE_SRC}`".to_string(),
                        "Run `systemd-tempfiles --remove --prefix=/nix/var/nix`".to_string(),
                        "Run `systemctl daemon-reload`".to_string(),
                    ],
                )]
            },
            InitSystem::Launchd => {
                vec![ActionDescription::new(
                    "Unconfigure Nix daemon related settings with launchctl".to_string(),
                    vec!["Run `launchctl unload {DARWIN_NIX_DAEMON_DEST}`".to_string()],
                )]
            },
            InitSystem::None => Vec::new(),
        }
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> Result<(), ActionError> {
        match self.init {
            InitSystem::Launchd => {
                execute_command(
                    Command::new("launchctl")
                        .process_group(0)
                        .arg("unload")
                        .arg(DARWIN_NIX_DAEMON_DEST),
                )
                .await
                .map_err(ActionError::Command)?;
            },
            InitSystem::Systemd => {
                // We separate stop and disable (instead of using `--now`) to avoid cases where the service isn't started, but is enabled.

                let socket_is_active = is_active("nix-daemon.socket").await?;
                let socket_is_enabled = is_enabled("nix-daemon.socket").await?;
                let service_is_active = is_active("nix-daemon.service").await?;
                let service_is_enabled = is_enabled("nix-daemon.service").await?;

                if socket_is_active {
                    execute_command(
                        Command::new("systemctl")
                            .process_group(0)
                            .args(["stop", "nix-daemon.socket"])
                            .stdin(std::process::Stdio::null()),
                    )
                    .await
                    .map_err(ActionError::Command)?;
                }

                if socket_is_enabled {
                    execute_command(
                        Command::new("systemctl")
                            .process_group(0)
                            .args(["disable", "nix-daemon.socket"])
                            .stdin(std::process::Stdio::null()),
                    )
                    .await
                    .map_err(ActionError::Command)?;
                }

                if service_is_active {
                    execute_command(
                        Command::new("systemctl")
                            .process_group(0)
                            .args(["stop", "nix-daemon.service"])
                            .stdin(std::process::Stdio::null()),
                    )
                    .await
                    .map_err(ActionError::Command)?;
                }

                if service_is_enabled {
                    execute_command(
                        Command::new("systemctl")
                            .process_group(0)
                            .args(["disable", "nix-daemon.service"])
                            .stdin(std::process::Stdio::null()),
                    )
                    .await
                    .map_err(ActionError::Command)?;
                }

                execute_command(
                    Command::new("systemd-tmpfiles")
                        .process_group(0)
                        .arg("--remove")
                        .arg("--prefix=/nix/var/nix")
                        .stdin(std::process::Stdio::null()),
                )
                .await
                .map_err(ActionError::Command)?;

                remove_file(TMPFILES_DEST)
                    .await
                    .map_err(|e| ActionError::Remove(PathBuf::from(TMPFILES_DEST), e))?;

                execute_command(
                    Command::new("systemctl")
                        .process_group(0)
                        .arg("daemon-reload")
                        .stdin(std::process::Stdio::null()),
                )
                .await
                .map_err(ActionError::Command)?;
            },
            InitSystem::None => {
                // Nothing here, no init
            },
        };

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigureNixDaemonServiceError {
    #[error("No supported init system found")]
    InitNotSupported,
}

async fn is_active(unit: &str) -> Result<bool, ActionError> {
    let output = Command::new("systemctl")
        .arg("is-active")
        .arg(unit)
        .output()
        .await
        .map_err(ActionError::Command)?;
    if String::from_utf8(output.stdout)?.starts_with("active") {
        tracing::trace!(%unit, "Is active");
        Ok(true)
    } else {
        tracing::trace!(%unit, "Is not active");
        Ok(false)
    }
}

async fn is_enabled(unit: &str) -> Result<bool, ActionError> {
    let output = Command::new("systemctl")
        .arg("is-enabled")
        .arg(unit)
        .output()
        .await
        .map_err(ActionError::Command)?;
    let stdout = String::from_utf8(output.stdout)?;
    if stdout.starts_with("enabled") || stdout.starts_with("linked") {
        tracing::trace!(%unit, "Is enabled");
        Ok(true)
    } else {
        tracing::trace!(%unit, "Is not enabled");
        Ok(false)
    }
}
