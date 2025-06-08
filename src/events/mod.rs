use std::{convert::Infallible, sync::Arc};

use axum::{Extension, body::Bytes, http::Response};
use error_stack::ResultExt;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use slack_morphism::prelude::*;
// use sqlx::SqlitePool;
use tracing::{debug, error, trace};

use crate::{
    BOT_TOKEN,
    models::{
        member::{Member, TriggeredMember},
        system::System,
        user,
    },
};

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
            if let Err(e) = push_event_callback(event, client, state).await {
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

async fn push_event_callback(
    event: SlackPushEventCallback,
    client: Arc<SlackHyperClient>,
    state: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match event.event {
        SlackEventCallbackBody::Message(message_event) => {
            debug!("Received message event!");
            trace!("Message: {:?}", message_event);

            let states = state.read().await;
            let user_state = states.get_user_state::<user::State>().unwrap();

            if message_event
                .subtype
                .as_ref()
                .is_some_and(|subtype| *subtype == SlackMessageEventType::MessageDeleted)
            {
                return Ok(());
            }

            let Some(user_id) = message_event.sender.user else {
                return Ok(());
            };

            let Some(mut system) =
                System::fetch_by_user_id(&user_state.db, &user::Id::new(user_id)).await?
            else {
                return Ok(());
            };

            let Some(ref channel_id) = message_event.origin.channel else {
                return Ok(());
            };

            let Some(content) = message_event.content else {
                return Ok(());
            };

            if let Some(ref message_content) = content.text {
                let Some(member) = system
                    .fetch_triggered_member(&user_state.db, message_content)
                    .await?
                else {
                    return Ok(());
                };

                debug!("Triggered member: {:#?}", member);

                if system.trigger_changes_active_member {
                    system
                        .change_active_member(Some(member.id), &user_state.db)
                        .await?;
                }

                rewrite_message(
                    &client,
                    channel_id,
                    message_event.origin.ts,
                    content,
                    member,
                    &system,
                    // &user_state.db,
                )
                .await?;

                return Ok(());
            }

            // No triggers ran, so check if there's any actively fronting member
            if let Some(member_id) = system.active_member_id {
                let Some(member) = Member::fetch_by_id(member_id, &user_state.db).await? else {
                    error!("Active member not found. This should not happen.");
                    return Ok(());
                };

                rewrite_message(
                    &client,
                    channel_id,
                    message_event.origin.ts,
                    content,
                    member.into(),
                    &system,
                    // &user_state.db,
                )
                .await?;
            }

            Ok(())
        }
        _ => Ok(()),
    }
}

async fn rewrite_message(
    client: &SlackHyperClient,
    channel_id: &SlackChannelId,
    message_id: SlackTs,
    mut content: SlackMessageContent,
    member: TriggeredMember,
    system: &System,
    // TODO: log this message in the db for future reference
    //  db: &SqlitePool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
        .collect::<Result<Vec<serde_json::Value>, serde_json::Error>>()?;

    blocks.extend(custom_image_blocks);

    let _res: SlackApiChatPostMessageResponse = bot_session
        .http_session_api
        .http_post(
            "chat.postMessage",
            &request,
            Some(&CHAT_POST_MESSAGE_SPECIAL_LIMIT_RATE_CTL),
        )
        .await
        .attach_printable("Error rewriting message")?;

    user_session
        .chat_delete(
            &SlackApiChatDeleteRequest::new(channel_id.clone(), message_id).with_as_user(true),
        )
        .await
        .attach_printable("Error deleting message")?;

    Ok(())
}

fn rewrite_content(content: &mut SlackMessageContent, member: &TriggeredMember) {
    debug!("Rewriting message content");

    if let Some(text) = &mut content.text {
        if member.is_prefix {
            if let Some(new_text) = text.strip_prefix(&member.trigger_text) {
                *text = new_text.to_string();
            }
        } else if let Some(new_text) = text.strip_suffix(&member.trigger_text) {
            *text = new_text.to_string();
        }
    }

    if let Some(blocks) = &mut content.blocks {
        for block in blocks {
            if let SlackBlock::RichText(richtext) = block {
                let elements = richtext["elements"].as_array_mut().unwrap();
                let len = elements.len();
                // the first and last elements would have the prefix and suffix respectively, so we can filter them
                let first = elements.get_mut(0).unwrap();

                if let Some(first_text) = first.pointer_mut("/elements/0/text") {
                    if member.is_prefix {
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
                    if !member.is_prefix {
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
