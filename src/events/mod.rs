//! Module containing all the event handlers for the Slack System Bot.
//!
//! This is where message rewriting, trigger detection, and message handling logic are implemented.

use std::{convert::Infallible, sync::Arc};

use axum::{Extension, body::Bytes, http::Response};
use error_stack::{Result, ResultExt};
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use slack_morphism::prelude::*;
use sqlx::SqlitePool;
use tracing::{debug, error, trace};

use crate::{
    BOT_TOKEN, fields,
    models::{self, trigger, user},
};

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum RewriteMessageError {
    /// Error while posting a message to Slack
    PostMessage,
    /// Error while deleting a message from Slack
    DeleteMessage,
    /// Error while serializing custom image blocks
    SerializeImageBlocks,
    /// Error while saving message log to database
    MessageLog,
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum PushEventError {
    /// Error while interacting with the Slack API
    SlackApi,
    /// Error while fetching system information from database
    SystemFetch,
    /// Error while fetching member information from database
    MemberFetch,
    /// Error while attempting to change the active member
    MemberChange,
    /// Error while attempting to rewrite the message
    MessageRewrite,
}

#[tracing::instrument(skip(environment, event))]
pub async fn process_push_event(
    Extension(environment): Extension<Arc<SlackHyperListenerEnvironment>>,
    Extension(event): Extension<SlackPushEvent>,
) -> Response<BoxBody<Bytes, Infallible>> {
    debug!("Received push event!");

    match event {
        SlackPushEvent::UrlVerification(url_verification) => {
            Response::new(Full::new(url_verification.challenge.into()).boxed())
        }
        SlackPushEvent::EventCallback(event) => {
            let client = environment.client.clone();
            let state = environment.user_state.clone();
            // https://rust-lang.github.io/rust-clippy/master/index.html#large_futures
            // Into the box you go
            if let Err(e) = Box::pin(push_event_callback(event, client, state)).await {
                error!("Error processing push event: {:#?}", e);
            }

            Response::new(Empty::new().boxed())
        }
        SlackPushEvent::AppRateLimited(rate_limited) => {
            trace!("Rate limited event: {:#?}", rate_limited);
            Response::new(Empty::new().boxed())
        }
    }
}

#[tracing::instrument(skip(event, state, client))]
async fn push_event_callback(
    event: SlackPushEventCallback,
    client: Arc<SlackHyperClient>,
    state: SlackClientEventsUserState,
) -> Result<(), PushEventError> {
    match event.event {
        SlackEventCallbackBody::Message(message_event)
            if message_event
                .subtype
                .as_ref()
                .is_some_and(|subtype| *subtype == SlackMessageEventType::MessageDeleted) =>
        {
            fields!(event_type = ?SlackMessageEventType::MessageDeleted, message_id = ?&message_event.deleted_ts, user = ?message_event.sender);
            let states = state.read().await;
            let user_state = states.get_user_state::<user::State>().unwrap();

            models::MessageLog::delete_by_message_id(
                &message_event.deleted_ts.unwrap(),
                &user_state.db,
            )
            .await
            .change_context(PushEventError::SlackApi)
            .attach_printable("Failed to delete message log")
            .map(|_| ())?;

            debug!("Message log deleted");
            Ok(())
        }
        SlackEventCallbackBody::Message(message_event)
            if message_event.subtype.is_none()
                || message_event
                    .subtype
                    .as_ref()
                    .is_some_and(|subtype| *subtype == SlackMessageEventType::MessageChanged) =>
        {
            handle_message(message_event, &client, &state).await
        }
        _ => Ok(()),
    }
}

#[tracing::instrument(skip(client, state))]
async fn handle_message(
    message_event: SlackMessageEvent,
    client: &SlackHyperClient,
    state: &SlackClientEventsUserState,
) -> error_stack::Result<(), PushEventError> {
    fields!(event_type = ?message_event.subtype);
    debug!("Received message event!");

    let states = state.read().await;
    let user_state = states.get_user_state::<user::State>().unwrap();

    let Some(user_id) = message_event.sender.user.map(user::Id::new) else {
        debug!("Failed to get user ID");
        return Ok(());
    };

    fields!(user_id = ?&user_id);

    let Some(mut system) = models::System::fetch_by_user_id(&user_id, &user_state.db)
        .await
        .change_context(PushEventError::SystemFetch)?
    else {
        debug!("Failed to fetch system");
        return Ok(());
    };

    fields!(system_id = %&system.id);

    let Some(ref channel_id) = message_event.origin.channel else {
        debug!("Failed to get channel ID");
        return Ok(());
    };

    fields!(channel_id = %&channel_id);

    let Some(content) = message_event.content else {
        debug!("Failed to get message content");
        return Ok(());
    };

    if let Some(ref message_content) = content.text {
        let Some(member) = system
            .find_member_by_trigger_rules(&user_state.db, message_content)
            .await
            .change_context(PushEventError::MemberFetch)?
        else {
            debug!("Member not triggered");
            return Ok(());
        };

        fields!(member = ?&member);
        debug!("Member triggered");

        if system.auto_switch_on_trigger {
            system
                .change_fronting_member(Some(member.id), &user_state.db)
                .await
                .change_context(PushEventError::MemberChange)?;
        }

        rewrite_message(
            client,
            channel_id,
            message_event.origin.ts,
            content,
            member,
            &system,
            &user_state.db,
        )
        .await
        .change_context(PushEventError::MessageRewrite)?;
        return Ok(());
    }

    debug!("Member not triggered");

    // No triggers ran, so check if there's any actively fronting member
    if let Some(member_id) = system.currently_fronting_member_id {
        fields!(member = %&member_id);
        let member = models::Member::fetch_by_id(member_id, &user_state.db)
            .await
            .change_context(PushEventError::MemberFetch)?;
        fields!(member = ?&member);

        rewrite_message(
            client,
            channel_id,
            message_event.origin.ts,
            content,
            member.into(),
            &system,
            &user_state.db,
        )
        .await
        .change_context(PushEventError::MemberFetch)?;
    }

    Ok(())
}

#[tracing::instrument(skip(client, db, system), fields(system_id = %system.id))]
async fn rewrite_message(
    client: &SlackHyperClient,
    channel_id: &SlackChannelId,
    message_id: SlackTs,
    mut content: SlackMessageContent,
    member: models::DetectedMember,
    system: &models::System,
    db: &SqlitePool,
) -> error_stack::Result<(), RewriteMessageError> {
    let token = SlackApiToken::new(system.slack_oauth_token.expose().into())
        .with_token_type(SlackApiTokenType::User);
    let user_session = client.open_session(&token);
    let bot_session = client.open_session(&BOT_TOKEN);

    rewrite_content(&mut content, &member);

    let mut custom_image_blocks = Vec::new();

    if let Some(files) = content.files.take() {
        #[derive(serde::Serialize)]
        struct CustomSlackFile {
            id: String,
        }

        #[derive(serde::Serialize)]
        struct CustomSlackImageBlock {
            #[serde(rename = "type")]
            typ: String,
            slack_file: CustomSlackFile,
            alt_text: String,
        }

        // update files to blocks
        let blocks = files
            .into_iter()
            .filter_map(|file| match file.filetype.map(|f| f.0).as_deref() {
                Some("png" | "jpg" | "jpeg" | "gif" | "webp") => {
                    // https://github.com/abdolence/slack-morphism-rust/issues/320
                    // Some(SlackImageBlock::new(file.permalink?, String::new()).into())

                    custom_image_blocks.push(CustomSlackImageBlock {
                        typ: "image".to_string(),
                        slack_file: CustomSlackFile {
                            id: file.id.0,
                        },
                        alt_text: String::new(),
                    });
                    None
                }
                Some("mp4" | "mpg" | "mpeg" | "mkv" | "avi" | "mov" | "ogv" | "wmv") => {
                    debug!("user uploaded a video. Can't really embed this.... Attaching to message as a rich content and calling it a day");
                    Some(SlackMarkdownBlock::new(format!("Video: [{}]({})", file.name?, file.permalink?)).into())
                }
                Some(typ) => {
                    debug!("unknown filetype {}. Don't know how to embed. Attaching to message as a rich content", typ);
                    Some(SlackMarkdownBlock::new(format!("File attachment: [{}]({})", file.name?, file.permalink?)).into())
                }
                None => None,
            });

        if let Some(slack_blocks) = content.blocks.as_mut() {
            slack_blocks.extend(blocks);
        } else {
            content.blocks = Some(blocks.collect());
        }
    }

    let message_request = SlackApiChatPostMessageRequest::new(channel_id.clone(), content)
        .with_username(member.display_name.clone())
        .opt_icon_url(member.profile_picture_url.clone());

    let mut request = serde_json::to_value(message_request).unwrap();

    let blocks = request.get_mut("blocks").unwrap().as_array_mut().unwrap();
    let custom_image_blocks = custom_image_blocks
        .into_iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<serde_json::Value>, serde_json::Error>>()
        .change_context(RewriteMessageError::SerializeImageBlocks)?;

    blocks.extend(custom_image_blocks);

    let res: SlackApiChatPostMessageResponse = bot_session
        .http_session_api
        .http_post(
            "chat.postMessage",
            &request,
            Some(&CHAT_POST_MESSAGE_SPECIAL_LIMIT_RATE_CTL),
        )
        .await
        .change_context(RewriteMessageError::PostMessage)?;

    models::MessageLog::insert(member.id, &res.ts, db)
        .await
        .change_context(RewriteMessageError::MessageLog)?;

    user_session
        .chat_delete(
            &SlackApiChatDeleteRequest::new(channel_id.clone(), message_id).with_as_user(true),
        )
        .await
        .change_context(RewriteMessageError::DeleteMessage)?;

    Ok(())
}

fn rewrite_content(content: &mut SlackMessageContent, member: &models::DetectedMember) {
    debug!("Rewriting message content");

    if let Some(text) = &mut content.text {
        match member.typ {
            trigger::Type::Prefix => {
                if let Some(new_text) = text.strip_prefix(&member.trigger_text) {
                    *text = new_text.to_string();
                }
            }
            trigger::Type::Suffix => {
                if let Some(new_text) = text.strip_suffix(&member.trigger_text) {
                    *text = new_text.to_string();
                }
            }
        }
    }

    if let Some(blocks) = &mut content.blocks {
        for block in blocks {
            if let SlackBlock::RichText(richtext) = block {
                let elements = richtext["elements"].as_array_mut().unwrap();
                let len = elements.len();
                // The first and last elements would have the prefix and suffix respectively, so we can filter them
                let first = elements.get_mut(0).unwrap();

                if let Some(first_text) = first.pointer_mut("/elements/0/text") {
                    if member.typ == trigger::Type::Prefix {
                        if let Some(new_text) = first_text
                            .as_str()
                            .and_then(|text| text.strip_prefix(&member.trigger_text))
                            .map(ToString::to_string)
                        {
                            *first_text = serde_json::Value::String(new_text);
                        }
                    }
                }

                let last = elements.get_mut(len - 1).unwrap();

                if let Some(last_text) = last.pointer_mut("/elements/0/text") {
                    if member.typ == trigger::Type::Suffix {
                        if let Some(new_text) = last_text
                            .as_str()
                            .and_then(|text| text.strip_suffix(&member.trigger_text))
                            .map(ToString::to_string)
                        {
                            *last_text = serde_json::Value::String(new_text);
                        }
                    }
                }
            }
        }
    }
}
