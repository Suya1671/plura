use std::sync::Arc;

use error_stack::{Result, ResultExt};
use oauth2::CsrfToken;
use slack_morphism::prelude::*;
use tokio::runtime::Handle;
use tracing::debug;

use crate::{
    models::{system, user},
    oauth::create_oauth_client,
};

#[derive(clap::Subcommand, Debug)]
pub enum System {
    /// Creates a system for your profile
    Create {
        /// The name of your system
        name: String,
    },
    /// Edits your system name
    Rename {
        /// Your system's new name
        name: String,
    },
    /// Reauthenticates your system with Slack
    Reauth,
    /// Get info about your or another user's system
    Info {
        /// The user to get info about (if left blank, defaults to you)
        user: Option<String>,
    },
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum CommandError {
    /// Error while calling the database
    Sqlx,
}

impl System {
    pub async fn run(
        self,
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        match self {
            Self::Create { name } => Self::create_system(event, state, name).await,
            Self::Rename { name } => Self::edit_system_name(event, state, name).await,
            Self::Info { user } => Self::get_system_info(event, client, state, user).await,
            Self::Reauth => Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("TODO: reauth".into()),
            )),
        }
    }

    async fn get_system_info(
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
        user: Option<String>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        debug!("Getting system info");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        // If the input exists, parse it into a user ID.
        // If it doesn't exist, use the user ID of the event.
        // There's probably a better way to write this behaviour but I'm not sure how.
        let Some(user_id) = user.map_or_else(
            || Some(event.user_id.clone().into()),
            |u| {
                user::parse_slack_user_id(&u).and_then(|id| {
                    Handle::current().block_on(async { id.trust(&client).await.ok() })
                })
            },
        ) else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Invalid user ID".into()),
            ));
        };

        let system = system::System::fetch_by_user_id(&user_state.db, &user_id)
            .await
            .change_context(CommandError::Sqlx)?;

        if let Some(system) = system {
            let fronting_member = system
                .active_member(&user_state.db)
                .await
                .change_context(CommandError::Sqlx)?;

            Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_blocks(slack_blocks![
                    some_into(
                        SlackSectionBlock::new()
                            .with_text(md!(format!("System name: {}", system.name)))
                    ),
                    some_into(SlackSectionBlock::new().with_text(md!(format!(
                            "Fronting member: {}",
                            fronting_member.map_or_else(
                                || "No fronting member".to_string(),
                                |m| m.display_name
                            )
                        ))))
                ]),
            ))
        } else {
            Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_blocks(slack_blocks![some_into(
                    SlackSectionBlock::new().with_text(md!("This user doesn't have a system!"))
                )]),
            ))
        }
    }

    async fn edit_system_name(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        name: String,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        debug!("Editing system name {name}");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        let Some(system_id) =
            system::System::fetch_by_user_id(&user_state.db, &event.user_id.into())
                .await
                .change_context(CommandError::Sqlx)?
                .map(|s| s.id)
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_blocks(slack_blocks![some_into(
                    SlackSectionBlock::new().with_text(md!(
                        "You don't have a system to edit! Create one with `/system create`"
                    ))
                )]),
            ));
        };

        sqlx::query!(
            r#"
            UPDATE systems
            SET name = $1
            WHERE id = $2
            "#,
            name,
            system_id
        )
        .execute(&user_state.db)
        .await
        .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Successfully updated system name!".into()),
        ))
    }

    async fn create_system(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        name: String,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        debug!("Creating system {name}");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        // todo: somehow remove this clone with cleaner code in the future`
        if system::System::fetch_by_user_id(&user_state.db, &user::Id::new(event.user_id.clone()))
            .await
            .change_context(CommandError::Sqlx)?
            .is_some()
        {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("You already have a system! If you need to reauthenticate, run /system reauth. If you need to change your system name, run /system rename".into()),
            ));
        }

        let oauth_client = create_oauth_client();

        // note: we aren't doing PKCE since this is only ran on a trusted server

        let (auth_url, csrf_token) = oauth_client
            .authorize_url(CsrfToken::new_random)
            // so we get a regular token as well. Required by oauth2 for some reason
            .add_extra_param("scope", "commands")
            .add_extra_param("user_scope", "users.profile:read,chat:write")
            .url();

        let secret = csrf_token.secret();

        sqlx::query!(
            r#"
            INSERT INTO system_oauth_process (name, owner_id, csrf)
            VALUES ($1, $2, $3)
            "#,
            name,
            event.user_id.0,
            secret
        )
        .execute(&user_state.db)
        .await
        .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(slack_blocks![some_into(
                SlackSectionBlock::new()
                    .with_text(md!("<{}|Finish creating your system>", auth_url))
            )]),
        ))
    }
}
