use std::sync::Arc;

mod alias;
mod member;
mod system;
mod trigger;

use alias::Alias;
use axum::{Extension, Json};
use clap::{Parser, error::ErrorKind};
use error_stack::ResultExt;
use slack_morphism::prelude::*;
use tracing::{Level, debug, error, trace};

use member::Member;
use system::System;
use trigger::Trigger;

use crate::fields;

#[derive(clap::Parser, Debug)]
#[command(color(clap::ColorChoice::Never))]
enum Command {
    #[clap(subcommand)]
    Members(Member),
    #[clap(subcommand)]
    System(System),
    #[clap(subcommand)]
    Triggers(Trigger),
    #[clap(subcommand)]
    Aliases(Alias),
}

impl Command {
    #[tracing::instrument(level = Level::DEBUG, skip(event, client, state), fields(runner_user_id = %event.user_id, runner_channel_id = %event.channel_id, runner_channel_name = ?event.channel_name, trigger_id = %event.trigger_id))]
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
                .change_context(CommandError::Members),
            Self::System(system) => system
                .run(event, client, state)
                .await
                .change_context(CommandError::System),
            Self::Triggers(triggers) => triggers
                .run(event, client, state)
                .await
                .change_context(CommandError::Triggers),
            Self::Aliases(aliases) => aliases
                .run(event, state)
                .await
                .change_context(CommandError::Aliases),
        }
    }
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
enum CommandError {
    /// Error running the members command
    Members,
    /// Error running the triggers command
    Triggers,
    /// Error running the system command
    System,
    /// Error running the aliases command
    Aliases,
}

// TO-DO: figure out error handling
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
            error!(error = ?e, "Error processing command event");
            Json(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Error processing command! Logged to developers".into()),
            ))
        }
    }
}

#[tracing::instrument(level = Level::TRACE, skip(client, state), fields(command))]
async fn command_event_callback(
    event: SlackCommandEvent,
    client: Arc<SlackHyperClient>,
    state: SlackClientEventsUserState,
) -> Result<SlackCommandEventResponse, CommandError> {
    trace!(command = ?event.command, "Received command");

    let formatted_command = event.command.0.trim_start_matches('/');
    let formatted = event.text.as_ref().map_or_else(
        || format!("slack-system-bot {formatted_command}"),
        |text| format!("slack-system-bot {formatted_command} {text}"),
    );

    fields!(command = &formatted);

    let parser = Command::try_parse_from(formatted.split_whitespace());

    match parser {
        Ok(parser) => {
            debug!(?parser, "Parsed command. Running...");
            let result = parser.run(event, client, state).await;
            match result {
                Ok(res) => {
                    debug!("Command executed successfully");
                    Ok(res)
                }
                Err(e) => {
                    error!(error = ?e, "Error running command");
                    Ok(SlackCommandEventResponse::new(
                        SlackMessageContent::new().with_text(
                            "Error running command! TODO: show error info on slack".into(),
                        ),
                    ))
                }
            }
        }
        Err(error) => {
            if !matches!(
                error.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                    | ErrorKind::DisplayVersion
            ) {
                debug!(error = ?error, "Error parsing command. Most likely user's fault");
            }

            let formatted = error.render();
            Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(formatted.to_string()),
            ))
        }
    }
}
