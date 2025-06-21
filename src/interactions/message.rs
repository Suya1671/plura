use error_stack::{Result, ResultExt};
use std::sync::Arc;
use tracing::{debug, warn};

use slack_morphism::prelude::*;

use crate::{
    BOT_TOKEN, fields,
    models::{message, user::State},
};

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum Error {
    /// Error while calling the Slack API
    Slack,
    /// Error while calling the database
    Sqlx,
    /// Unable to parse view
    ParsingView,
}

#[tracing::instrument(skip_all, fields(trigger_id = ?event.trigger_id))]
pub async fn start_edit(
    event: SlackInteractionMessageActionEvent,
    client: Arc<SlackHyperClient>,
    user_state: &State,
) -> Result<(), Error> {
    let session = client.open_session(&BOT_TOKEN);
    let message = event
        .message
        .expect("Expected message to edit to, well, have a message");

    let Some(log) = message::MessageLog::fetch_by_message_id(&message.origin.ts, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        debug!(
            "Message not found in database. User is trying to edit a message that isn't sent by us. Send an error back"
        );

        session
            .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                event.channel.unwrap().id,
                event.user.id,
                SlackMessageContent::new().with_text(
                    "This message was not sent by a member! Did you maybe want to reproxy instead?"
                        .into(),
                ),
            ))
            .await
            .change_context(Error::Slack)?;

        return Ok(());
    };

    let system = log
        .member_id
        .fetch(&user_state.db)
        .await
        .change_context(Error::Sqlx)?
        .system_id
        .fetch(&user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    if system.owner_id != event.user.id {
        debug!("User is not the owner of the system");

        session
            .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                event.channel.unwrap().id,
                event.user.id,
                SlackMessageContent::new().with_text("This message was not sent by you!".into()),
            ))
            .await
            .change_context(Error::Slack)?;

        return Ok(());
    }

    let message_content = message.content.text.unwrap_or_default();

    let view = EditMessageView {
        message: message_content,
    }
    .create_view(&message.origin.ts, &event.channel.unwrap().id);

    fields!(view = ?&view);

    session
        .views_open(&SlackApiViewsOpenRequest::new(event.trigger_id, view))
        .await
        .change_context(Error::Slack)?;

    debug!("Opened view");

    Ok(())
}

#[tracing::instrument(skip(client, user_state))]
pub async fn edit(
    view_state: SlackViewState,
    client: &SlackHyperClient,
    user_state: &State,
    user_id: SlackUserId,
    message_id: SlackTs,
    channel_id: SlackChannelId,
) -> Result<(), Error> {
    let session = client.open_session(&BOT_TOKEN);

    let Some(log) = message::MessageLog::fetch_by_message_id(&message_id, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        debug!(
            "Message not found in database. User is trying to edit a message that isn't sent by us. Send an error back"
        );

        let conversation = session
            .conversations_open(
                &SlackApiConversationsOpenRequest::new().with_users(vec![user_id.clone()]),
            )
            .await
            .change_context(Error::Slack)?
            .channel;

        session
            .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                conversation.id,
                user_id.clone(),
                SlackMessageContent::new().with_text(
                    "This message was not sent by a member! Did you maybe want to reproxy instead?"
                        .into(),
                ),
            ))
            .await
            .change_context(Error::Slack)?;

        return Ok(());
    };

    let system = log
        .member_id
        .fetch(&user_state.db)
        .await
        .change_context(Error::Sqlx)?
        .system_id
        .fetch(&user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    if system.owner_id != user_id {
        debug!("User is not the owner of the system");

        let conversation = session
            .conversations_open(
                &SlackApiConversationsOpenRequest::new().with_users(vec![user_id.clone()]),
            )
            .await
            .change_context(Error::Slack)?
            .channel;

        session
            .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                conversation.id,
                user_id.clone(),
                SlackMessageContent::new().with_text("This message was not sent by you!".into()),
            ))
            .await
            .change_context(Error::Slack)?;

        return Ok(());
    }

    let view = EditMessageView::try_from(view_state).change_context(Error::ParsingView)?;

    fields!(view = ?&view);

    session
        .chat_update(&SlackApiChatUpdateRequest::new(
            channel_id,
            SlackMessageContent::new().with_text(view.message),
            message_id,
        ))
        .await
        .change_context(Error::Slack)?;

    debug!("Edited message");

    Ok(())
}

#[derive(Debug, Default, Clone)]
pub struct EditMessageView {
    pub message: String,
}

impl EditMessageView {
    /// Due to the way the slack blocks are created, all fields are moved.
    /// Clone the whole struct if you need to keep the original.
    pub fn create_blocks(self) -> Vec<SlackBlock> {
        slack_blocks![some_into(SlackInputBlock::new(
            // https://github.com/abdolence/slack-morphism-rust/issues/327
            "Message (No rich text support. Sorry!)".into(),
            SlackBlockPlainTextInputElement::new("message".into())
                .with_initial_value(self.message)
                .into(),
        ))]
    }

    pub fn create_view(self, message_id: &SlackTs, channel_id: &SlackChannelId) -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Edit message".into(), self.create_blocks())
                .with_submit("Edit".into())
                .with_external_id(format!("edit_message_{}_{}", message_id.0, channel_id.0)),
        )
    }
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
/// A field was missing from the view
pub struct MissingFieldError(String);

impl TryFrom<SlackViewState> for EditMessageView {
    type Error = MissingFieldError;

    fn try_from(value: SlackViewState) -> std::result::Result<Self, Self::Error> {
        let mut view = Self::default();
        for (_id, values) in value.values {
            for (id, content) in values {
                match &*id.0 {
                    "message" => {
                        view.message = content
                            .value
                            .ok_or_else(|| MissingFieldError("message".to_string()))?;
                    }
                    other => {
                        warn!("Unknown field in view when parsing a member::View: {other}");
                    }
                }
            }
        }

        if view.message.is_empty() {
            return Err(MissingFieldError("message".to_string()));
        }

        Ok(view)
    }
}
