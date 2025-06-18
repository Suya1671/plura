use std::sync::Arc;

use error_stack::{Result, ResultExt, report};
use slack_morphism::prelude::*;
use tracing::{debug, info, trace};

use crate::{
    BOT_TOKEN, fields,
    models::{
        self,
        member::{self, View},
        system::ChangeActiveMemberError,
        user,
    },
};

#[derive(clap::Subcommand, Debug)]
pub enum Member {
    /// Adds a new member to your system. Expect a popup to fill in the member info!
    Add,
    /// Deletes a member from your system. Use the member id from /member list
    Delete {
        /// The member to delete
        member: i64,
    },
    /// Gets info about a member
    Info {
        /// The member to get info about. Use the member id from /member list
        member_id: i64,
    },
    /// Lists all members in a system
    List {
        /// The system to list members from. If left blank, defaults to your system.
        system: Option<String>,
    },
    /// Edits a member's info
    Edit {
        /// The member to edit. Use the member id from /member list. Expect a popup to edit the info!
        member_id: i64,
    },
    /// Switch to a different member
    #[group(required = true)]
    Switch {
        /// The member to switch to. Use the member id from /member list
        #[clap(group = "member")]
        member_id: Option<i64>,
        /// Don't switch to another member, just message with the base account
        #[clap(long, short, action, group = "member", alias = "none")]
        base: bool,
    },
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum CommandError {
    /// Error while calling the Slack API
    SlackApi,
    /// Error while calling the database
    Sqlx,
}

impl Member {
    #[tracing::instrument(skip_all)]
    pub async fn run(
        self,
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running members command");
        match self {
            Self::Add => {
                let token = &BOT_TOKEN;
                let session = client.open_session(token);
                Self::create_member(event, session).await
            }
            Self::Delete { member } => {
                debug!(member_id = member, "Delete member command not implemented");
                Ok(SlackCommandEventResponse::new(
                    SlackMessageContent::new().with_text("Working on it".into()),
                ))
            }
            Self::Info { member_id } => Self::member_info(event, &state, member_id).await,
            Self::Edit { member_id } => {
                Self::edit_member(event, client.open_session(&BOT_TOKEN), &state, member_id).await
            }
            Self::List { system } => Self::list_members(event, state, system).await,
            Self::Switch { member_id, base } => {
                Self::switch_member(event, state, member_id, base).await
            }
        }
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    async fn switch_member(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        member_id: Option<i64>,
        base: bool,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Switching member");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        let Some(mut system) =
            models::System::fetch_by_user_id(&user_state.db, &event.user_id.into())
                .await
                .change_context(CommandError::Sqlx)?
        else {
            debug!("User has no system configured");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("You don't have a system yet!".into()),
            ));
        };

        fields!(system_id = %system.id);
        debug!("Found user system");

        let new_active_member_id = if base {
            None
        } else {
            let member_id =
                member_id.expect("member_id to be Some, as the clap rules require it to be.");
            debug!(requested_member_id = member_id, "Validating member ID");

            member::Id::new(member_id)
                .validate_by_system(system.id, &user_state.db)
                .await
                .ok()
        };

        debug!(target_member_id = ?new_active_member_id, "Changing active member");

        let new_member = system
            .change_active_member(new_active_member_id, &user_state.db)
            .await;

        let response = match new_member {
            Ok(Some(member)) => {
                info!(member_name = %member.full_name, member_id = %member.id, "Successfully switched to member");
                format!("Switch to member {}", member.full_name)
            }
            Ok(None) => {
                info!("Successfully switched to base account");
                "Switched to base account".into()
            }
            Err(ChangeActiveMemberError::MemberNotFound) => {
                debug!("Requested member not found in system");
                "The member you gave doesn't exist!".into()
            }
            Err(ChangeActiveMemberError::Sqlx(err)) => {
                return Err(report!(err).change_context(CommandError::Sqlx));
            }
        };

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text(response),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(user_id, system_id))]
    async fn list_members(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        system: Option<String>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Listing all members");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        // If the input exists, parse it into a user ID
        // If it doesn't exist, use the user ID of the event.
        // If the user ID is invalid, return an error.
        // There's probably a better way to write this behaviour but I'm not sure how.
        let Some((user_id, is_author)) = system.map_or_else(
            || Some((user::Id::new(event.user_id), true)),
            |u| user::parse_slack_user_id(&u).map(|id| (id, false)),
        ) else {
            debug!("Invalid user ID provided in system parameter");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Invalid user ID".into()),
            ));
        };

        fields!(user_id = %user_id.clone());

        let Some(system) = models::System::fetch_by_user_id(&user_state.db, &user_id)
            .await
            .change_context(CommandError::Sqlx)?
        else {
            debug!(target_user_id = %user_id, is_self = is_author, "Target user has no system");
            return if is_author {
                Ok(SlackCommandEventResponse::new(
                    SlackMessageContent::new().with_text("You don't have a system yet!".into()),
                ))
            } else {
                Ok(SlackCommandEventResponse::new(
                    SlackMessageContent::new().with_text("This user doesn't have a system!".into()),
                ))
            };
        };

        fields!(system_id = %system.id);

        let members = system
            .get_members(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        debug!(member_count = members.len(), "Retrieved system members");

        let member_blocks = members
            .into_iter()
            .map(|member| {
                let fields = [
                    Some(md!("Display Name: {}", member.display_name)),
                    Some(md!("Member ID: {}", member.id)),
                    member.title.as_ref().map(|title| md!("Title: {}", title)),
                    member
                        .pronouns
                        .as_ref()
                        .map(|pronouns| md!("Pronouns: {}", pronouns)),
                    member
                        .name_pronunciation
                        .as_ref()
                        .map(|name_pronunciation| {
                            md!("Name Pronunciation: {}", name_pronunciation)
                        }),
                    Some(md!("Created At: {}", member.created_at)),
                ]
                .into_iter()
                .flatten()
                .collect();

                SlackSectionBlock::new()
                    .with_text(md!("Name: {}", member.full_name))
                    .with_fields(fields)
            })
            .map(Into::into)
            .collect();

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(member_blocks),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(user_id = %event.user_id, system_id, member_id))]
    async fn member_info(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member_id: i64,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member info command");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let member_id = member::Id::new(member_id);

        let Some(system_id) =
            models::System::fetch_by_user_id(&user_state.db, &event.user_id.into())
                .await
                .change_context(CommandError::Sqlx)?
                .map(|system| system.id)
        else {
            debug!("User has no system configured");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(
                    "You don't have a system yet! Make one with `/system create <name>`".into(),
                ),
            ));
        };

        fields!(system_id = %system_id);

        let Some(member) =
            models::Member::fetch_by_and_trust_id(system_id, member_id, &user_state.db)
                .await
                .change_context(CommandError::Sqlx)?
        else {
            debug!("Member not found");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Member not found. Make sure you used the correct ID".into()),
            ));
        };

        fields!(member_id = %member.id);
        debug!("Member found");

        let fields = [
            Some(md!("Display Name: {}", member.display_name)),
            Some(md!("Member ID: {}", member.id)),
            member.title.as_ref().map(|title| md!("Title: {}", title)),
            member
                .pronouns
                .as_ref()
                .map(|pronouns| md!("Pronouns: {}", pronouns)),
            member
                .name_pronunciation
                .as_ref()
                .map(|name_pronunciation| md!("Name Pronunciation: {}", name_pronunciation)),
        ]
        .into_iter()
        .flatten()
        .collect();

        let block = SlackSectionBlock::new()
            .with_text(md!("Name: {}", member.full_name))
            .with_fields(fields);

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(vec![block.into()]),
        ))
    }

    #[tracing::instrument(skip(event, session), fields(view_id))]
    async fn create_member(
        event: SlackCommandEvent,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member creation command");
        let view = View::create_add_view();

        let view = session
            .views_open(&SlackApiViewsOpenRequest::new(event.trigger_id, view))
            .await
            .attach_printable("Error opening view")
            .change_context(CommandError::SlackApi)?;

        info!(view_id = %view.view.state_params.id, "Successfully opened member creation view");

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("View opened!".into()),
        ))
    }

    #[tracing::instrument(skip(event, session, state), fields(user_id = %event.user_id, trigger_id = %event.trigger_id))]
    async fn edit_member(
        event: SlackCommandEvent,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
        state: &SlackClientEventsUserState,
        member_id: i64,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member edit command");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let user_id = user::Id::new(event.user_id);
        let member_id = member::Id::new(member_id);

        let Some(system_id) = models::System::fetch_by_user_id(&user_state.db, &user_id)
            .await
            .change_context(CommandError::Sqlx)?
            .map(|system| system.id)
        else {
            debug!("User has no system configured");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(
                    "You don't have a system yet! Make one with `/system create <name>`".into(),
                ),
            ));
        };

        let Some(member) =
            models::Member::fetch_by_and_trust_id(system_id, member_id, &user_state.db)
                .await
                .change_context(CommandError::Sqlx)?
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Member not found. Make sure you used the correct ID".into()),
            ));
        };

        let member_id = member.id;

        let view = member::View::from(member).create_edit_view(member_id);

        let view = session
            .views_open(&SlackApiViewsOpenRequest::new(
                event.trigger_id.clone(),
                view,
            ))
            .await
            .attach_printable("Error opening view")
            .change_context(CommandError::SlackApi)?;

        info!(view_id = %view.view.state_params.id, member_id = %member_id, "Successfully opened member edit view");

        Ok(SlackCommandEventResponse::new(SlackMessageContent::new()))
    }
}
