use crate::fetch_system;
use std::sync::Arc;

use error_stack::{Result, ResultExt};
use oauth2::CsrfToken;
use slack_morphism::prelude::*;
use tokio::runtime::Handle;
use tracing::{debug, trace};

use crate::{
    fields,
    models::{self, user},
    oauth::create_oauth_client,
};

#[derive(clap::Subcommand, Debug)]
#[clap(verbatim_doc_comment)]
/// A system is your plural system: a collection of members/profiles.
///
/// You can use /system create to get started with your system, and thus access the rest of the bots features.
/// This will require you to authenticate with Slack, as the bot will need to delete messages from your account when
/// sending messages as members.
///
/// Also see:
/// - /members for getting started with your members and their profiles.
pub enum System {
    /// Creates a system for your profile
    Create,
    /// Re-authenticates your system with Slack
    Reauth,
    /// Get information about your or another user's system
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
    #[tracing::instrument(skip_all)]
    pub async fn run(
        self,
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        match self {
            Self::Create => Self::create_system(event, state).await,
            Self::Info { user } => Self::get_system_info(event, client, state, user).await,
            Self::Reauth => Self::reauth(event, state).await,
        }
    }

    async fn reauth(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Reauthenticating system");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);
        let system = system_id
            .fetch(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;
        let oauth_client = create_oauth_client();

        let (auth_url, csrf_token) = oauth_client
            .authorize_url(CsrfToken::new_random)
            // So we get a regular token as well. Required by oauth2 for some reason
            .add_extra_param("scope", "commands")
            .add_extra_param("user_scope", "users.profile:read,chat:write")
            .url();

        let secret = csrf_token.secret();

        sqlx::query!(
            r#"
            INSERT INTO system_oauth_process (owner_id, csrf)
            VALUES ($1, $2)
            ON CONFLICT (owner_id) DO UPDATE SET csrf = $2
            "#,
            system.owner_id,
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

    #[tracing::instrument(skip_all, fields(user_id, system_id))]
    async fn get_system_info(
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
        user: Option<String>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Getting system info");

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

        fields!(user_id = %&user_id);
        trace!("Mapped user ID");

        let system = models::System::fetch_by_user_id(&user_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        if let Some(system) = system {
            fields!(system_id = %system.id);
            debug!("Fetched system");
            let fronting_member = system
                .active_member(&user_state.db)
                .await
                .change_context(CommandError::Sqlx)?;

            Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_blocks(slack_blocks![some_into(
                    SlackSectionBlock::new().with_text(md!(format!(
                        "Fronting member: {}",
                        fronting_member
                            .map_or_else(|| "No fronting member".to_string(), |m| m.display_name)
                    )))
                )]),
            ))
        } else {
            debug!("User does not have a system");
            Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_blocks(slack_blocks![some_into(
                    SlackSectionBlock::new().with_text(md!("This user doesn't have a system!"))
                )]),
            ))
        }
    }

    #[tracing::instrument(skip(event, state))]
    async fn create_system(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Creating system");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let user_id = user::Id::new(event.user_id);

        if let Some(system) = models::System::fetch_by_user_id(&user_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        {
            debug!(system_id = %system.id, "User already has a system");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(
                    "You already have a system! If you need to reauthenticate, run /system reauth."
                        .into(),
                ),
            ));
        }

        let oauth_client = create_oauth_client();

        // Note: we aren't doing PKCE since this is only ran on a trusted server

        let (auth_url, csrf_token) = oauth_client
            .authorize_url(CsrfToken::new_random)
            // So we get a regular token as well. Required by oauth2 for some reason
            .add_extra_param("scope", "commands")
            .add_extra_param("user_scope", "users.profile:read,chat:write")
            .url();

        let secret = csrf_token.secret();

        sqlx::query!(
            r#"
            INSERT INTO system_oauth_process (owner_id, csrf)
            VALUES ($1, $2)
            ON CONFLICT (owner_id) DO UPDATE SET csrf = $2
            "#,
            user_id.id,
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

#[macro_export]
/// Fetches the system ID associated with the user who triggered the command.
/// Also attaches the system ID to context
///
/// Else, returns early with a warning message
macro_rules! fetch_system {
    ($event:expr, $user_state:expr => $system_var_name:ident) => {
        let Some($system_var_name) = $crate::models::System::fetch_by_user_id(
            &$crate::models::user::Id::new($event.user_id),
            &$user_state.db,
        )
        .await
        .change_context(CommandError::Sqlx)?
        .map(|system| system.id) else {
            use slack_morphism::prelude::*;

            ::tracing::debug!("User does not have a system");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(
                    "You don't have a system yet! Make one with `/system create`".into(),
                ),
            ));
        };

        $crate::fields!(system_id = %$system_var_name);
        ::tracing::debug!("Fetched system");
    };
}
