use error_stack::{Result, ResultExt};
use slack_morphism::prelude::*;
use tracing::debug;

use crate::{
    fetch_member, fetch_system,
    models::{self, alias, member::MemberRef, trust::Untrusted, user},
};

#[derive(clap::Subcommand, Debug)]
#[clap(verbatim_doc_comment)]
/// An alias is a unique identifier for a member within a system.
///
/// You can use aliases to refer to members without knowing their member ID in other commands.
///
/// Also see:
/// - /members for managing members and their profiles.
pub enum Alias {
    /// Adds a new alias for a member.
    Add {
        /// The member to add the alias for. Use either an existing alias or member ID
        member: MemberRef,
        /// The alias to add. Must be unique for the system. Cannot be just a number
        alias: String,
    },
    /// Deletes an alias
    Delete {
        /// The alias to delete. Use the alias ID from /alias list
        alias: alias::Id<Untrusted>,
    },
    /// Lists all of your systems aliases
    List {
        /// If specified, lists the aliases for the given member.
        member: Option<MemberRef>,
    },
    /// Edit an alias
    Edit {
        /// The alias to edit. Use the alias ID from /alias list
        alias: alias::Id<Untrusted>,
        /// The new alias to set. Must be unique for the system. Cannot be just a number
        new_alias: String,
    },
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
/// Errors that can occur when running the alias command.
pub enum CommandError {
    /// Error while calling the database
    Sqlx,
}

impl Alias {
    #[tracing::instrument(skip_all)]
    pub async fn run(
        self,
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        match self {
            Self::Add { member, alias } => Self::create_alias(event, &state, member, alias).await,
            Self::Delete { alias } => Self::delete_alias(event, &state, alias).await,
            Self::List { member } => Self::list_aliases(event, &state, member).await,
            Self::Edit { alias, new_alias } => {
                Self::edit_alias(event, &state, alias, new_alias).await
            }
        }
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    async fn create_alias(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member: MemberRef,
        alias: String,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        debug!("Creating alias");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        fetch_member!(member, user_state, system_id => member_id);

        if alias.parse::<i64>().is_ok() {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(
                    "Alias cannot be a valid integer, as it could be mistaken for a member ID."
                        .to_string(),
                ),
            ));
        }

        models::Alias::insert(member_id, system_id, alias, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Alias created successfully.".to_string()),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    async fn delete_alias(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        alias: alias::Id<Untrusted>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        debug!("Deleting alias");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        let Some(alias) = alias
            .validate_by_system(system_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Alias not found.".to_string()),
            ));
        };

        alias
            .delete(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Alias deleted successfully.".to_string()),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    async fn list_aliases(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member: Option<MemberRef>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        debug!("Listing aliases");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        let aliases = if let Some(member) = member {
            debug!("Fetching aliases by member");
            fetch_member!(member, user_state, system_id => member_id);

            models::Alias::fetch_by_member_id(member_id, &user_state.db)
                .await
                .change_context(CommandError::Sqlx)?
        } else {
            models::Alias::fetch_by_system_id(system_id, &user_state.db)
                .await
                .change_context(CommandError::Sqlx)?
        };

        if aliases.is_empty() {
            debug!("No aliases found");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("No aliases found.".into()),
            ));
        }

        debug!(len = aliases.len(), "Found aliases");

        let alias_blocks = aliases
            .into_iter()
            .map(|alias| {
                let fields = vec![
                    md!("Member ID: {}", alias.member_id),
                    md!("Alias: {}", alias.alias),
                ];

                SlackSectionBlock::new()
                    .with_text(md!("*Alias {}*", alias.id))
                    .with_fields(fields)
            })
            .map(Into::into)
            .collect();

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(alias_blocks),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    async fn edit_alias(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        alias: alias::Id<Untrusted>,
        new_alias: String,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        debug!("Editing alias");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        let Some(alias) = alias
            .validate_by_system(system_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Alias not found.".to_string()),
            ));
        };

        alias
            .change_alias(new_alias, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Alias updated successfully.".to_string()),
        ))
    }
}
