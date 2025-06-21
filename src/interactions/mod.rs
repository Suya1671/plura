mod member;
use std::error::Error;
use std::sync::Arc;

use axum::Extension;
use error_stack::Report;
use member::{create_member, edit_member};
use slack_morphism::prelude::*;
use tracing::{debug, error};

use crate::BOT_TOKEN;
use crate::models::{self, Trusted, user};

#[tracing::instrument(skip(event, environment))]
pub async fn process_interaction_event(
    Extension(environment): Extension<Arc<SlackHyperListenerEnvironment>>,
    Extension(event): Extension<SlackInteractionEvent>,
) {
    let client = environment.client.clone();
    let states = environment.user_state.clone();

    if let Err(error) = interaction_event(client, event, states).await {
        error!(?error, "Error processing interaction event");
    }
}

#[tracing::instrument(skip(client, event, states))]
async fn interaction_event(
    client: Arc<SlackHyperClient>,
    event: SlackInteractionEvent,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match event {
        SlackInteractionEvent::ViewSubmission(slack_interaction_view_submission_event) => {
            handle_view_submission(slack_interaction_view_submission_event, client, states).await
        }
        SlackInteractionEvent::Shortcut(shortcut) => {
            debug!(?shortcut, "Received shortcut event");
            todo!()
        }
        event => {
            debug!(?event, "Received interaction event",);
            Ok(())
        }
    }
}

async fn handle_view_submission(
    view_submission: SlackInteractionViewSubmissionEvent,
    client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match view_submission.view.view {
        SlackView::Home(view) => {
            debug!(?view, "Received home view");
            Ok(())
        }
        SlackView::Modal(view) => {
            debug!(?view, "Received modal view");

            let user_id: user::Id<Trusted> = view_submission.user.id.into();

            let Some(view_state) = view_submission.view.state_params.state else {
                error!("No state found in modal view submission");
                return Ok(());
            };

            handle_modal_view(client, view, view_state, states, user_id).await;

            Ok(())
        }
    }
}

#[tracing::instrument(skip(client, view, states))]
async fn handle_modal_view(
    client: Arc<SlackHyperClient>,
    view: SlackModalView,
    view_state: SlackViewState,
    states: SlackClientEventsUserState,
    user_id: user::Id<Trusted>,
) {
    let states = states.read().await;
    let user_state = states.get_user_state::<user::State>().unwrap();
    let external_id = view.external_id.as_deref();

    match external_id {
        None => {
            error!(
                "No external id found in modal view. To the person that created the modal: How do you expect the bot to figure out what to do?"
            );
        }
        Some("create_member") => {
            debug!("Received create member modal view");

            if let Err(error) =
                create_member(view_state, &client, user_state, user_id.clone()).await
            {
                handle_user_error(error, user_id.into(), client).await;
            }
        }
        Some(id) if id.starts_with("edit_member_") => {
            debug!("Received edit member modal view");

            let Ok(member_id) = id
                .strip_prefix("edit_member_")
                .expect("id starts with edit_member_")
                .parse::<i64>()
                .map(models::member::Id::new)
            else {
                error!(
                    id,
                    "Failed to parse member id from external id. Bailing in case this was a malicious call",
                );
                return;
            };

            // TO-DO: better handling of Err case
            let Ok(Some(trusted_member_id)) =
                member_id.validate_by_user(&user_id, &user_state.db).await
            else {
                error!(
                    id,
                    "Failed to validate member id from external id. Bailing in case this was a malicious call",
                );
                return;
            };

            if let Err(error) = edit_member(
                view_state,
                &client,
                user_state,
                user_id.clone(),
                trusted_member_id,
            )
            .await
            {
                handle_user_error(error, user_id.into(), client).await;
            }
        }
        Some(id) => {
            error!("receieved unknown external id: {id}");
        }
    }
}

pub async fn handle_user_error<E>(
    error: Report<E>,
    user: SlackUserId,
    client: Arc<SlackHyperClient>,
) where
    E: std::error::Error + Send + Sync + 'static,
{
    error!(?error);

    let session = client.open_session(&BOT_TOKEN);

    let conversation = session
        .conversations_open(&SlackApiConversationsOpenRequest::new().with_users(vec![user.clone()]))
        .await
        .expect("Expected to be able to open conversation")
        .channel;

    session
        .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
            conversation.id,
            user,
            SlackMessageContent::new().with_text(format!("An error occured! {error}",)),
        ))
        .await
        .expect("Expected to be able to post ephemeral message");
}
