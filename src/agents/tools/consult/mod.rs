use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agents::tools::{ToolContext, VizierTool};
use crate::error::VizierError;
use crate::schema::{
    AgentConfig, AgentId, TopicId, VizierChannelId, VizierRequest, VizierRequestContent,
    VizierResponse, VizierResponseContent, VizierSession,
};
use crate::transport::VizierTransport;

pub struct ConsultAgent {
    agent_id: String,
    agents: HashMap<String, AgentConfig>,
    transport: VizierTransport,
}

impl ConsultAgent {
    pub fn new(agent_id: AgentId, agents: HashMap<String, AgentConfig>, transport: VizierTransport) -> Self {
        Self {
            agent_id,
            agents,
            transport,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ConsultAgentArgs {
    #[schemars(
        description = "[optional] identifier for current topic/conversation, the consult session will be ephemeral if left empty"
    )]
    pub topic_id: Option<TopicId>,
    #[schemars(description = "agent_id of the target agent")]
    pub agent_id: String,
    #[schemars(description = "Question, task, or discussion to ask the agent")]
    pub prompt: String,
}

#[async_trait::async_trait]
impl VizierTool for ConsultAgent {
    type Input = ConsultAgentArgs;
    type Output = String;

    fn name() -> String {
        "consult_agent".to_string()
    }

    fn description(&self) -> String {
        "Consult, or ask other agent and wait for the response".into()
    }

    async fn call(&self, args: Self::Input, _ctx: &ToolContext) -> Result<Self::Output, VizierError> {
        let (response_tx, response_rx) = flume::unbounded();

        let curr_session = VizierSession(
            args.agent_id.clone(),
            VizierChannelId::InterAgent(vec![self.agent_id.clone(), args.agent_id.clone()]),
            args.topic_id,
        );

        let _ = self
            .transport
            .send_request(
                curr_session.clone(),
                VizierRequest {
                    timestamp: chrono::Utc::now(),
                    user: self.agent_id.clone(),
                    content: VizierRequestContent::Chat(args.prompt.clone()),
                    metadata: json!({}),

                    ..Default::default()
                },
                Some(response_tx),
            )
            .await
            .map_err(|err| VizierError(err.to_string()))?;

        // Bounded wait. Without this the tool blocks until the caller's own
        // prompt timeout, which is an hour by default - the person asking sees
        // nothing at all in the meantime and assumes the bot is broken.
        let wait = std::time::Duration::from_secs(
            std::env::var("VIZIER_CONSULT_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(120),
        );
        let deadline = tokio::time::Instant::now() + wait;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(format!(
                    "{} did not answer in time. Answer it yourself instead of waiting.",
                    args.agent_id
                ));
            }

            let received = match tokio::time::timeout(remaining, response_rx.recv_async()).await {
                Err(_) => {
                    return Ok(format!(
                        "{} did not answer in time. Answer it yourself instead of waiting.",
                        args.agent_id
                    ));
                }
                Ok(Err(err)) => {
                    // Channel closed: the other agent went away mid-request.
                    return Ok(format!(
                        "{} could not be reached ({}). Answer it yourself.",
                        args.agent_id, err
                    ));
                }
                Ok(Ok(r)) => r,
            };

            match received.content {
                VizierResponseContent::Message { content, .. } => return Ok(content),
                // Anything terminal that is not a message would otherwise spin
                // here until the deadline for no reason.
                VizierResponseContent::Error { kind, message } => {
                    return Ok(format!(
                        "{} failed ({:?}: {}). Answer it yourself.",
                        args.agent_id, kind, message
                    ));
                }
                VizierResponseContent::Empty => {
                    return Ok(format!(
                        "{} returned nothing. Answer it yourself.",
                        args.agent_id
                    ));
                }
                // Thinking/tool-progress frames: keep waiting.
                _ => continue,
            }
        }
    }
}

pub struct DelegateAgent {
    agent_id: String,
    agents: HashMap<String, AgentConfig>,
    transport: VizierTransport,
}

impl DelegateAgent {
    pub fn new(agent_id: AgentId, agents: HashMap<String, AgentConfig>, transport: VizierTransport) -> Self {
        Self {
            agent_id,
            agents,
            transport,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DelegateAgentArgs {
    #[schemars(description = "agent_id of the target agent")]
    pub agent_id: String,
    #[schemars(description = "task for the agent")]
    pub prompt: String,
}

#[async_trait::async_trait]
impl VizierTool for DelegateAgent {
    type Input = DelegateAgentArgs;
    type Output = String;

    fn name() -> String {
        "delegate_agent".to_string()
    }

    fn description(&self) -> String {
        let available_agents_desc = self
            .agents
            .iter()
            .map(|(agent_id, config)| {
                format!(
                    r#"**Agent ID:** {}
**Name:** {}
**Description:** {}"#,
                    agent_id,
                    config.name,
                    config.description.clone().unwrap_or("".into())
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Assign an agent a task to do, this is a non-blocking tool, you won't need to wait the agent\n\nAvailable Agent\n{available_agents_desc}"
        )
    }

    async fn call(&self, args: Self::Input, _ctx: &ToolContext) -> Result<Self::Output, VizierError> {
        let target_agent = args.agent_id.clone();
        let curr_session = VizierSession(
            args.agent_id.clone(),
            VizierChannelId::InterAgent(vec![self.agent_id.clone(), args.agent_id.clone()]),
            None,
        );

        self.transport
            .send_request(
                curr_session.clone(),
                VizierRequest {
                    timestamp: chrono::Utc::now(),
                    user: self.agent_id.clone(),
                    content: VizierRequestContent::Prompt(args.prompt),
                    metadata: json!({}),
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(|err| VizierError(err.to_string()))?;

        Ok(format!("Task delegated to agent '{}'", target_agent))
    }
}
