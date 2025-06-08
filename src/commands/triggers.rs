use std::sync::Arc;

use error_stack::{Result, ResultExt};
use slack_morphism::prelude::*;
use tracing::debug;

use crate::{
    BOT_TOKEN,
    models::{
        member::{self, Member},
        system::System,
        trigger, user,
    },
};

#[derive(clap::Subcommand, Debug)]
pub enum Triggers {
    /// Adds a new trigger for a member. Expect a popup to fill in the info!
    Add {
        /// The member to add the trigger for. Use the member id from /member list
        member: i64,
    },
    /// Deletes a trigger
    Delete {
        /// The trigger to delete. Use the trigger id from /trigger list
        id: i64,
    },
    /// Lists all of your triggers
    List {
        /// If specified, lists the triggers for the given member. Use the member id from /member list
        member: Option<i64>,
    },
    /// Edit a trigger
    Edit {
        /// The trigger to edit. Use the trigger id from /trigger list
        id: i64,
    },
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum CommandError {
    /// Error while calling the Slack API
    Slack,
    /// Error while calling the database
    Sqlx,
}

impl Triggers {
    pub async fn run(
        self,
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        match self {
            Self::Add { member } => {
                let token = &BOT_TOKEN;
                let session = client.open_session(token);
                Self::create_trigger(event, &state, session, member).await
            }
            Self::Delete { id } => Self::delete_trigger(event, &state, id).await,
            Self::List { member } => Self::list_triggers(event, &state, member).await,
            Self::Edit { id } => {
                let token = &BOT_TOKEN;
                let session = client.open_session(token);
                Self::edit_trigger(event, &state, session, id).await
            }
        }
    }

    async fn create_trigger(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
        member_id: i64,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let member_id = member::Id::new(member_id);

        let Some(system_id) =
            System::fetch_by_user_id(&user_state.db, &user::Id::new(event.user_id))
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

        let Some(member_id) = Member::fetch_by_and_trust_id(system_id, member_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
            .map(|member| member.id)
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Member not found. Make sure you used the correct ID".into()),
            ));
        };

        let view = trigger::View::new(String::new(), true).create_add_view(member_id);
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

    pub async fn delete_trigger(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        trigger_id: i64,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let trigger_id = trigger::Id::new(trigger_id);

        let Some(system_id) =
            System::fetch_by_user_id(&user_state.db, &user::Id::new(event.user_id))
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

        // Validate the trigger belongs to the user's system
        let Ok(trigger_id) = trigger_id
            .validate_by_system(system_id, &user_state.db)
            .await
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Trigger not found. Make sure you used the correct ID".into()),
            ));
        };

        // Fetch the trigger to delete it
        trigger_id
            .delete(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Successfully deleted trigger!".into()),
        ))
    }

    pub async fn list_triggers(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member_id: Option<i64>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        let Some(system_id) =
            System::fetch_by_user_id(&user_state.db, &user::Id::new(event.user_id))
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

        let triggers = if let Some(member_id) = member_id {
            let member_id = member::Id::new(member_id);

            // Validate the member belongs to the user's system
            let Ok(member_id) = member_id
                .validate_by_system(system_id, &user_state.db)
                .await
            else {
                return Ok(SlackCommandEventResponse::new(
                    SlackMessageContent::new()
                        .with_text("Member not found. Make sure you used the correct ID".into()),
                ));
            };

            member_id
                .fetch_triggers(&user_state.db)
                .await
                .change_context(CommandError::Sqlx)?
        } else {
            system_id
                .list_triggers(&user_state.db)
                .await
                .change_context(CommandError::Sqlx)?
        };

        if triggers.is_empty() {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("No triggers found.".into()),
            ));
        }

        let trigger_blocks = triggers
            .into_iter()
            .map(|trigger| {
                let fields = vec![
                    md!("Trigger ID: {}", trigger.id),
                    md!("Member ID: {}", trigger.member_id),
                    md!(
                        "{}: {}",
                        if trigger.is_prefix {
                            "Prefix"
                        } else {
                            "Suffix"
                        },
                        trigger.text
                    ),
                ];

                SlackSectionBlock::new()
                    .with_text(md!("**Trigger {}**", trigger.id))
                    .with_fields(fields)
            })
            .map(Into::into)
            .collect();

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(trigger_blocks),
        ))
    }

    pub async fn edit_trigger(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
        trigger_id: i64,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();
        let trigger_id = trigger::Id::new(trigger_id);

        let Some(system_id) =
            System::fetch_by_user_id(&user_state.db, &user::Id::new(event.user_id))
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

        // Validate the trigger belongs to the user's system
        let Ok(trigger_id) = trigger_id
            .validate_by_system(system_id, &user_state.db)
            .await
        else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Trigger not found. Make sure you used the correct ID".into()),
            ));
        };

        // Fetch the trigger to edit
        let trigger = trigger::Trigger::fetch_by_id(trigger_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        let Some(trigger) = trigger else {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Trigger not found. Make sure you used the correct ID".into()),
            ));
        };

        let view = trigger::View::from(trigger).create_edit_view(trigger_id);

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
