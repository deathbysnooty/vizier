use std::sync::Arc;

use anyhow::Result;

#[cfg(feature = "docker-shell")]
use crate::agents::shell::docker::DockerShell;
use crate::{config::shell::ShellConfig, agents::shell::local::LocalShell};

#[cfg(feature = "docker-shell")]
pub mod docker;
pub mod local;

#[async_trait::async_trait]
pub trait ShellProvider {
    async fn exec(&self, commands: String) -> Result<String>;
}

pub struct VizierShell(Arc<Box<dyn ShellProvider + Sync + Send + 'static>>);

impl VizierShell {
    pub fn build<Shell: ShellProvider + Sync + Send + 'static>(shell: Shell) -> Self {
        Self(Arc::new(Box::new(shell)))
    }

    pub async fn new(config: &ShellConfig) -> Result<Self> {
        Ok(match config {
            #[cfg(feature = "docker-shell")]
            ShellConfig::Docker(docker) => Self::build(DockerShell::new(docker.clone()).await?),
            // Deliberately an error and not a fall-through to the local shell: the
            // point of asking for the `docker` environment is that the agent's
            // commands run in a container, so a build without that backend must
            // refuse rather than silently run them on the host.
            #[cfg(not(feature = "docker-shell"))]
            ShellConfig::Docker(_) => {
                return Err(anyhow::anyhow!(
                    "This build has no docker shell, so the shell tool cannot run commands in a \
                     container. Build with --features docker-shell to enable it. Refusing to fall \
                     back to the local shell - switch the agent's shell environment to `local` \
                     explicitly if running its commands directly on the host is what you want."
                ));
            }
            ShellConfig::Local(local) => Self::build(LocalShell::new(local.clone()).await?),
        })
    }
}

#[async_trait::async_trait]
impl ShellProvider for VizierShell {
    async fn exec(&self, commands: String) -> Result<String> {
        self.0.exec(commands).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config naming the docker environment must keep parsing in every build,
    /// with or without the backend compiled in.
    #[test]
    fn docker_shell_config_still_parses() {
        let config: ShellConfig = serde_json::from_str(
            r#"{"environment":"docker","image":{"source":"pull","name":"ubuntu:latest"},"container_name":"vizier"}"#,
        )
        .expect("a docker shell config should still parse");
        assert!(matches!(config, ShellConfig::Docker(_)));
    }

    /// Without the docker backend compiled in, asking for it has to fail. It must
    /// never quietly become a local shell: that would run the agent's commands on
    /// the host, which is the one thing choosing docker was meant to prevent.
    #[cfg(not(feature = "docker-shell"))]
    #[tokio::test]
    async fn docker_shell_is_refused_never_downgraded_to_local() {
        use crate::config::shell::DockerShellConfig;

        let err = VizierShell::new(&ShellConfig::Docker(DockerShellConfig::default()))
            .await
            .err()
            .expect("a build without the docker shell must refuse a docker shell config");
        assert!(err.to_string().contains("--features docker-shell"));
    }
}
