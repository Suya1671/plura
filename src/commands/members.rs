use std::sync::Arc;

use error_stack::{Result, ResultExt, report};
use slack_morphism::prelude::*;
use tracing::{debug, info};

use crate::{
    BOT_TOKEN,
    commands::members,
    models::{
        member::{self, Member, View},
        system::{ChangeActiveMemberError, System},
        user,
    },
};

#[derive(clap::Subcommand, Debug)]
pub enum Members {
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
    Slack,
    /// Error while calling the database
    Sqlx,
}

impl Members {
    pub async fn run(
        self,
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        match self {
            Self::Add => {
                let token = &BOT_TOKEN;
                let session = client.open_session(token);
                Self::create_member(event, session).await
            }
            Self::Delete { member } => {
                info!("Deleting member {member}");
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

    async fn switch_member(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        member_id: Option<i64>,
        base: bool,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        let Some(mut system) = System::fetch_by_user_id(&user_state.db, &event.user_id.into())
            .await
            .change_context(CommandError::Sqlx)?
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("You don't have a system yet!".into()),
            ));
        };

        let new_active_member_id = if base {
            None
        } else {
            member::Id::new(
                member_id.expect("member_id to be Some, as the clap rules require it to be."),
            )
            .validate_by_system(system.id, &user_state.db)
            .await
            .ok()
        };

        let new_member = system
            .change_active_member(new_active_member_id, &user_state.db)
            .await;

        let response = match new_member {
            Ok(Some(member)) => format!("Switch to member {}", member.full_name),
            Ok(None) => "Switched to base account".into(),
            Err(ChangeActiveMemberError::MemberNotFound) => {
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

    async fn list_members(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        system: Option<String>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        // If the input exists, parse it into a user ID
        // If it doesn't exist, use the user ID of the event.
        // If the user ID is invalid, return an error.
        // Theres probably a better way to write this behaviour but I'm not sure how.
        let Some((user_id, is_author)) = system.map_or_else(
            || Some((user::Id::new(event.user_id), true)),
            |u| user::parse_slack_user_id(&u).map(|id| (id, false)),
        ) else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Invalid user ID".into()),
            ));
        };

        let Some(system) = System::fetch_by_user_id(&user_state.db, &user_id)
            .await
            .change_context(CommandError::Sqlx)?
        else {
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

        let members = system
            .get_members(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

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

    async fn member_info(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member_id: i64,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let member_id = member::Id::new(member_id);

        let Some(system_id) = System::fetch_by_user_id(&user_state.db, &event.user_id.into())
            .await
            .change_context(CommandError::Sqlx)?
            .map(|system| system.id)
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(
                    "You don't have a system yet! Make one with `/system create <name>`".into(),
                ),
            ));
        };

        let Some(member) = Member::fetch_by_and_trust_id(system_id, member_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Member not found. Make sure you used the correct ID".into()),
            ));
        };

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

    async fn create_member(
        event: SlackCommandEvent,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let view = View::create_add_view();

        let view = session
            .views_open(&SlackApiViewsOpenRequest::new(event.trigger_id, view))
            .await
            .attach_printable("Error opening view")
            .change_context(CommandError::Slack)?;

        debug!("Opened view: {:#?}", view);

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("View opened!".into()),
        ))
    }

    async fn edit_member(
        event: SlackCommandEvent,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
        state: &SlackClientEventsUserState,
        member_id: i64,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let user_id = user::Id::new(event.user_id);
        let member_id = member::Id::new(member_id);

        let Some(system_id) = System::fetch_by_user_id(&user_state.db, &user_id)
            .await
            .change_context(CommandError::Sqlx)?
            .map(|system| system.id)
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(
                    "You don't have a system yet! Make one with `/system create <name>`".into(),
                ),
            ));
        };

        let Some(member) = Member::fetch_by_and_trust_id(system_id, member_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Member not found. Make sure you used the correct ID".into()),
            ));
        };

        let member_id = member.id;

        let view = members::View::from(member).create_edit_view(member_id);

        let view = session
            .views_open(&SlackApiViewsOpenRequest::new(
                event.trigger_id.clone(),
                view,
            ))
            .await
            .attach_printable("Error opening view")
            .change_context(CommandError::Slack)?;

        debug!("Opened view: {:#?}", view);

        Ok(SlackCommandEventResponse::new(SlackMessageContent::new()))
    }
}
