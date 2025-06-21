use error_stack::{Result, ResultExt};
use slack_morphism::prelude::*;
use tracing::debug;

use crate::{
    fetch_member, fetch_system, fields,
    models::{self, Untrusted, member::MemberRef, trigger, user},
};

#[derive(clap::Subcommand, Debug)]
pub enum Trigger {
    /// Adds a new trigger for a member. Expect a popup to fill in the info!
    Add {
        /// The member to add the trigger for.
        member: MemberRef,
        /// The type of trigger
        #[clap(name = "type")]
        typ: trigger::Type,
        /// The trigger content
        content: String,
    },
    /// Deletes a trigger
    Delete {
        /// The trigger to delete.
        id: trigger::Id<Untrusted>,
    },
    /// Lists all of your triggers
    List {
        /// If specified, lists the triggers for the given member.
        member: Option<MemberRef>,
    },
    /// Edit a trigger
    Edit {
        /// The trigger to edit. Use the trigger id from /trigger list
        id: trigger::Id<Untrusted>,
        /// The type of trigger
        #[clap(name = "type", long = "type", short)]
        typ: Option<trigger::Type>,
        /// The trigger content
        #[clap(long, short)]
        content: Option<String>,
    },
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum CommandError {
    /// Error while calling the database
    Sqlx,
}

impl Trigger {
    #[tracing::instrument(skip_all)]
    pub async fn run(
        self,
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        match self {
            Self::Add {
                member,
                typ,
                content,
            } => Self::create_trigger(event, &state, member, typ, content).await,
            Self::Delete { id } => Self::delete_trigger(event, &state, id).await,
            Self::List { member } => Self::list_triggers(event, &state, member).await,
            Self::Edit { id, typ, content } => {
                Self::edit_trigger(event, &state, id, typ, content).await
            }
        }
    }

    #[tracing::instrument(skip(event, state), fields(system_id, member_id))]
    async fn create_trigger(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member_id: MemberRef,
        typ: trigger::Type,
        content: String,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);
        fetch_member!(member_id, user_state, system_id => member_id);

        models::Trigger::insert(member_id, system_id, typ, content, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Trigger created!".into()),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    pub async fn delete_trigger(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        trigger_id: trigger::Id<Untrusted>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        let Some(system_id) =
            models::System::fetch_by_user_id(&user_state.db, &user::Id::new(event.user_id))
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

        fields!(system_id = %system_id);

        // Validate the trigger belongs to the user's system
        let Ok(trigger_id) = trigger_id
            .validate_by_system(system_id, &user_state.db)
            .await
        else {
            debug!("Trigger not found");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("Trigger not found. Make sure you used the correct ID".into()),
            ));
        };

        trigger_id
            .delete(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Deleted trigger!".into()),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    pub async fn list_triggers(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member_ref: Option<MemberRef>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        let triggers = if let Some(member_ref) = member_ref {
            fetch_member!(member_ref, user_state, system_id => member_id);

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
            debug!("No triggers found");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("No triggers found.".into()),
            ));
        }

        debug!(len = triggers.len(), "Found triggers");

        let trigger_blocks = triggers
            .into_iter()
            .map(|trigger| {
                let fields = vec![
                    md!("Member ID: {}", trigger.member_id),
                    md!("{}: {}", trigger.typ, trigger.text),
                ];

                SlackSectionBlock::new()
                    .with_text(md!("*Trigger {}*", trigger.id))
                    .with_fields(fields)
            })
            .map(Into::into)
            .collect();

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(trigger_blocks),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    pub async fn edit_trigger(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        trigger_id: trigger::Id<Untrusted>,
        typ: Option<trigger::Type>,
        text: Option<String>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

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

        fields!(trigger_id = %trigger_id);

        trigger_id
            .update(typ, text, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Updated trigger!".into()),
        ))
    }
}
