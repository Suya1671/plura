use std::sync::Arc;

mod members;
mod system;
mod triggers;
use axum::{Extension, Json};
use clap::Parser;
use error_stack::ResultExt;
use members::Members;

use slack_morphism::prelude::*;
use system::System;
use tracing::{debug, error};
use triggers::Triggers;

#[derive(clap::Parser, Debug)]
#[command(color(clap::ColorChoice::Never))]
enum Command {
    #[clap(subcommand)]
    Members(Members),
    #[clap(subcommand)]
    System(System),
    #[clap(subcommand)]
    Triggers(Triggers),
}

impl Command {
    pub async fn run(
        self,
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
    ) -> error_stack::Result<SlackCommandEventResponse, CommandError> {
        match self {
            Self::Members(members) => members
                .run(event, client, state)
                .await
                .attach_printable("Failed to run members command")
                .change_context(CommandError::Command),
            Self::System(system) => system
                .run(event, client, state)
                .await
                .attach_printable("Failed to run system command")
                .change_context(CommandError::Command),
            Self::Triggers(triggers) => triggers
                .run(event, client, state)
                .await
                .attach_printable("Failed to run triggers command")
                .change_context(CommandError::Command),
        }
    }
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
enum CommandError {
    /// Error running the command
    Command,
}

// TODO: figure out error handling
#[tracing::instrument(skip(environment, event))]
pub async fn process_command_event(
    Extension(environment): Extension<Arc<SlackHyperListenerEnvironment>>,
    Extension(event): Extension<SlackCommandEvent>,
) -> Json<SlackCommandEventResponse> {
    let client = environment.client.clone();
    let state = environment.user_state.clone();

    match command_event_callback(event, client, state).await {
        Ok(response) => Json(response),
        Err(e) => {
            error!("Error processing command event: {:#?}", e);
            Json(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Error processing command! Logged to developers".into()),
            ))
        }
    }
}

async fn command_event_callback(
    event: SlackCommandEvent,
    client: Arc<SlackHyperClient>,
    state: SlackClientEventsUserState,
) -> Result<SlackCommandEventResponse, CommandError> {
    debug!("Received command: {:?}", event.command);

    let formatted_command = event.command.0.trim_start_matches('/');
    let formatted = event.text.as_ref().map_or_else(
        || format!("slack-system-bot {formatted_command}"),
        |text| format!("slack-system-bot {formatted_command} {text}"),
    );

    debug!("Formatted command: {formatted}");

    let parser = Command::try_parse_from(formatted.split_whitespace());

    match parser {
        Ok(parser) => {
            debug!("Parsed command: {:?}", parser);
            let result = parser.run(event, client, state).await;
            match result {
                Ok(res) => {
                    debug!("Command {} executed successfully", formatted);
                    Ok(res)
                }
                Err(e) => {
                    error!("Error running command {formatted}");
                    error!("{e:?}");
                    Ok(SlackCommandEventResponse::new(
                        SlackMessageContent::new().with_text(
                            "Error running command! TODO: show error info on slack".into(),
                        ),
                    ))
                }
            }
        }
        Err(error) => {
            let formatted = error.render();
            Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(formatted.to_string()),
            ))
        }
    }
}
