use error_stack::{Result, ResultExt};
use std::sync::Arc;
use tracing::{debug, warn};

use slack_morphism::prelude::*;

use crate::{
    BOT_TOKEN, fields,
    models::{
        Member, MessageLog, System, member,
        trust::Trusted,
        user::{self, State},
    },
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

    let Some(log) = MessageLog::fetch_by_message_id(&message.origin.ts, &user_state.db)
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

    let Some(log) = MessageLog::fetch_by_message_id(&message_id, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        warn!(
            "Message not found in database. User is trying to edit a message that isn't sent by us. Bailing since this shouldn't happen"
        );
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
        warn!("User is not the owner of the system. This shouldn't happen. Bailing");
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

#[tracing::instrument(skip_all, fields(trigger_id = ?event.trigger_id))]
pub async fn start_reproxy(
    event: SlackInteractionMessageActionEvent,
    client: Arc<SlackHyperClient>,
    user_state: &State,
) -> Result<(), Error> {
    let message = event
        .message
        .as_ref()
        .expect("Expected message to reproxy to, well, have a message");

    match MessageLog::fetch_by_message_id(&message.origin.ts, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    {
        Some(log) => start_reproxy_log(log, event, client, user_state).await?,
        None => start_reproxy_user(event, client, user_state).await?,
    }
    Ok(())
}

#[tracing::instrument(skip(client, user_state))]
async fn start_reproxy_user(
    event: SlackInteractionMessageActionEvent,
    client: Arc<SlackHyperClient>,
    user_state: &State,
) -> Result<(), Error> {
    let session = client.open_session(&BOT_TOKEN);
    let message = event
        .message
        .expect("Expected message to reproxy to, well, have a message");

    let Some(user_id) = message.sender.user.filter(|user| *user == event.user.id) else {
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
    };

    let user_id: user::Id<Trusted> = user_id.into();

    let Some(system) = System::fetch_by_user_id(&user_id, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        debug!("System not found for user");

        session
                .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                    event.channel.unwrap().id,
                    event.user.id,
                    SlackMessageContent::new().with_text("System not found! Make sure you have a system set up. You can use /system create to create one.".into()),
                ))
                .await
                .change_context(Error::Slack)?;

        return Ok(());
    };

    let members = system
        .members(&user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    let view = ReproxyView { member: None }.create_view(
        &members,
        &message.origin.ts,
        &event.channel.unwrap().id,
    );

    fields!(view = ?&view);

    session
        .views_open(&SlackApiViewsOpenRequest::new(event.trigger_id, view))
        .await
        .change_context(Error::Slack)?;

    debug!("Opened view");

    Ok(())
}

#[tracing::instrument(skip(event, client, user_state))]
async fn start_reproxy_log(
    log: MessageLog,
    event: SlackInteractionMessageActionEvent,
    client: Arc<SlackHyperClient>,
    user_state: &State,
) -> Result<(), Error> {
    let session = client.open_session(&BOT_TOKEN);

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

    let members = system
        .members(&user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    let view = ReproxyView {
        member: Some(log.member_id.id),
    }
    .create_view(&members, &log.message_id, &event.channel.unwrap().id);

    fields!(view = ?&view);

    session
        .views_open(&SlackApiViewsOpenRequest::new(event.trigger_id, view))
        .await
        .change_context(Error::Slack)?;

    debug!("Opened view");

    Ok(())
}

pub async fn reproxy(
    view_state: SlackViewState,
    client: &SlackHyperClient,
    user_state: &State,
    user_id: SlackUserId,
    message_id: SlackTs,
    channel_id: SlackChannelId,
) -> Result<(), Error> {
    let session = client.open_session(&BOT_TOKEN);

    let view = ReproxyView::try_from(view_state).change_context(Error::ParsingView)?;
    fields!(view = ?&view);

    let Some(id) = view.member.map(member::Id::new) else {
        warn!("Missing member on view. This should not happen. bailing");
        return Ok(());
    };

    let Some(system) = System::fetch_by_user_id(&user_id.into(), &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        warn!("System not found for user. This should not happen. bailing");
        return Ok(());
    };

    let Some(id) = id
        .validate_by_system(system.id, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        warn!("Member not found in database. This should not happen. bailing");
        return Ok(());
    };

    let member = id.fetch(&user_state.db).await.change_context(Error::Sqlx)?;

    let Ok(messages) = session
        .conversations_history(
            &SlackApiConversationsHistoryRequest::new()
                .with_channel(channel_id.clone())
                .with_latest(message_id.clone())
                .with_limit(1)
                .with_inclusive(true),
        )
        .await
    else {
        warn!("Failed to fetch message history");
        return Ok(());
    };

    let Some(message) = messages.messages.first() else {
        warn!(?messages, "Message not found in history");
        return Ok(());
    };

    let message_request =
        SlackApiChatPostMessageRequest::new(channel_id.clone(), message.content.clone())
            .with_username(member.display_name.clone())
            .opt_icon_url(member.profile_picture_url.clone());

    session
        .chat_post_message(&message_request)
        .await
        .change_context(Error::Slack)?;

    let token = SlackApiToken::new(system.slack_oauth_token.expose().into())
        .with_token_type(SlackApiTokenType::User);

    let user_session = client.open_session(&token);

    user_session
        .chat_delete(&SlackApiChatDeleteRequest::new(channel_id, message_id))
        .await
        .change_context(Error::Slack)?;

    debug!("Reproxied message");

    Ok(())
}

#[derive(Debug, Default, Clone)]
pub struct ReproxyView {
    pub member: Option<i64>,
}

impl ReproxyView {
    /// Due to the way the slack blocks are created, all fields are moved.
    /// Clone the whole struct if you need to keep the original.
    pub fn create_blocks(self, members: &[Member]) -> Vec<SlackBlock> {
        let options = members
            .iter()
            .map(|member| {
                SlackBlockChoiceItem::<SlackBlockPlainTextOnly>::new(
                    format!(
                        "{} ({}, ID: {})",
                        member.display_name, member.full_name, member.id
                    )
                    .into(),
                    member.id.to_string(),
                )
            })
            .collect();

        let value = self.member.and_then(|member_id| {
            members
                .iter()
                .find(|member| member.id.id == member_id)
                .map(|member| {
                    SlackBlockChoiceItem::<SlackBlockPlainTextOnly>::new(
                        format!(
                            "{} ({}, ID: {})",
                            member.display_name, member.full_name, member.id
                        )
                        .into(),
                        member.id.to_string(),
                    )
                })
        });

        slack_blocks![some_into(
            SlackSectionBlock::new()
                .with_text(SlackBlockText::Plain("Member".into()))
                .with_accessory(
                    SlackBlockStaticSelectElement::new("member".into())
                        .with_options(options)
                        .opt_initial_option(value)
                        .into()
                )
        )]
    }
    pub fn create_view(
        self,
        members: &[Member],
        message_id: &SlackTs,
        channel_id: &SlackChannelId,
    ) -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Reproxy message".into(), self.create_blocks(members))
                .with_submit("Reproxy".into())
                .with_external_id(format!("reproxy_message_{}_{}", message_id.0, channel_id.0)),
        )
    }
}

impl TryFrom<SlackViewState> for ReproxyView {
    type Error = MissingFieldError;

    fn try_from(value: SlackViewState) -> std::result::Result<Self, Self::Error> {
        let mut view = Self::default();

        for (_id, values) in value.values {
            for (id, content) in values {
                match &*id.0 {
                    "member" => {
                        view.member = content
                            .selected_option
                            .and_then(|option| option.value.parse::<i64>().ok());
                    }
                    other => {
                        warn!("Unknown field in view when parsing a member::View: {other}");
                    }
                }
            }
        }

        if view.member.is_none() {
            return Err(MissingFieldError("member".to_string()));
        }

        Ok(view)
    }
}

#[tracing::instrument(skip(client, user_state))]
pub async fn delete(
    event: SlackInteractionMessageActionEvent,
    client: Arc<SlackHyperClient>,
    user_state: &State,
) -> Result<(), Error> {
    let session = client.open_session(&BOT_TOKEN);

    let message = event
        .message
        .expect("Expected message to edit to, well, have a message");

    let Some(log) = MessageLog::fetch_by_message_id(&message.origin.ts, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        debug!(
            "Message not found in database. User is trying to delete a message that isn't sent by us."
        );

        session
            .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                event.channel.unwrap().id,
                event.user.id,
                SlackMessageContent::new().with_text("A member didn't send this message.".into()),
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
        session
            .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                event.channel.unwrap().id,
                event.user.id,
                SlackMessageContent::new()
                    .with_text("Your system didn't send this message.".into()),
            ))
            .await
            .change_context(Error::Slack)?;

        return Ok(());
    }

    session
        .chat_delete(&SlackApiChatDeleteRequest::new(
            event.channel.unwrap().id,
            message.origin.ts,
        ))
        .await
        .change_context(Error::Slack)?;

    debug!("Deleted message");

    Ok(())
}

#[tracing::instrument(skip(client, user_state))]
pub async fn info(
    event: SlackInteractionMessageActionEvent,
    client: Arc<SlackHyperClient>,
    user_state: &State,
) -> Result<(), Error> {
    let session = client.open_session(&BOT_TOKEN);

    let message = event
        .message
        .expect("Expected message to edit to, well, have a message");

    let Some(log) = MessageLog::fetch_by_message_id(&message.origin.ts, &user_state.db)
        .await
        .change_context(Error::Sqlx)?
    else {
        debug!(
            "Message not found in database. User is trying to get information about a message that isn't sent by us."
        );

        session
            .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
                event.channel.unwrap().id,
                event.user.id,
                SlackMessageContent::new().with_text("A member didn't send this message.".into()),
            ))
            .await
            .change_context(Error::Slack)?;

        return Ok(());
    };

    let member = log
        .member_id
        .fetch(&user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    let system = member
        .system_id
        .fetch(&user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    let blocks = slack_blocks![
        some_into(SlackHeaderBlock::new(member.full_name.into())),
        some_into(SlackDividerBlock::new()),
        some_into(
            SlackSectionBlock::new()
                .with_text(md!(
                    "*{}*\n{}{}\n*System*: {} ({})",
                    member.display_name,
                    member.pronouns.unwrap_or_default(),
                    member
                        .name_pronunciation
                        .map(|pronunciation| format!(" - {pronunciation}"))
                        .unwrap_or_default(),
                    system.name,
                    system.owner_id.to_slack_format()
                ))
                .opt_accessory(member.profile_picture_url.and_then(|url| Some(
                    SlackSectionBlockElement::Image(SlackBlockImageElement::new(
                        url.parse().ok()?,
                        "Profile picture".into()
                    ))
                )))
        ),
        optionally_into(system.currently_fronting_member_id.is_some_and(|id| id == member.id) => SlackSectionBlock::new().with_text(md!("*Fronting*")))
        // TO-DO: fields
    ];

    session
        .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
            event.channel.unwrap().id,
            event.user.id,
            SlackMessageContent::new().with_blocks(blocks),
        ))
        .await
        .change_context(Error::Slack)?;

    debug!("Deleted message");

    Ok(())
}
