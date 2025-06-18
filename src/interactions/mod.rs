mod member;
mod trigger;
use std::error::Error;
use std::sync::Arc;

use axum::Extension;
use error_stack::ResultExt;
use member::{create_member, edit_member};
use slack_morphism::prelude::*;
use tracing::{debug, error};
use trigger::{create_trigger, edit_trigger};

use crate::fields;
use crate::models::system::System;
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
            match slack_interaction_view_submission_event.view.view {
                SlackView::Home(view) => {
                    debug!(?view, "Received home view");
                    Ok(())
                }
                SlackView::Modal(ref view) => {
                    debug!(?view, "Received modal view");

                    let user_id: user::Id<Trusted> =
                        slack_interaction_view_submission_event.user.id.into();
                    let states = states.read().await;
                    let user_state = states.get_user_state::<user::State>().unwrap();

                    fields!(user_id = %&user_id);

                    let Some(view_state) = slack_interaction_view_submission_event
                        .view
                        .state_params
                        .state
                    else {
                        error!("No state found in view submission");
                        return Ok(());
                    };

                    handle_modal_view(
                        client,
                        view_state,
                        user_state,
                        user_id,
                        view.external_id.as_deref(),
                    )
                    .await
                }
            }
        }
        event => {
            debug!("Received interaction event: {:#?}", event);
            Ok(())
        }
    }
}

#[tracing::instrument(skip(client, view_state, user_state))]
async fn handle_modal_view(
    client: Arc<SlackHyperClient>,
    view_state: SlackViewState,
    user_state: &user::State,
    user_id: user::Id<Trusted>,
    external_id: Option<&str>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match external_id {
        None => {
            error!(
                "No external id found in modal view. To the person that created the modal: How do you expect the bot to figure out what to do?"
            );
            Ok(())
        }
        Some("create_member") => {
            debug!("Received create member modal view");

            create_member(view_state, &client, user_state, user_id).await?;

            Ok(())
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
                return Ok(());
            };

            let Some(trusted_member_id) =
                member_id.validate_by_user(&user_id, &user_state.db).await?
            else {
                error!(
                    id,
                    "Failed to validate member id from external id. Bailing in case this was a malicious call",
                );
                return Ok(());
            };

            edit_member(view_state, &client, user_state, user_id, trusted_member_id).await?;

            Ok(())
        }
        Some(id) if id.starts_with("create_trigger_") => {
            debug!("Creating trigger");

            let member_id = id
                .strip_prefix("create_trigger_")
                .expect("Failed to parse member id from external id")
                .parse::<i64>()
                .map(models::member::Id::new)
                .expect("Failed to parse member id from external id");

            let Some(trusted_member_id) =
                member_id.validate_by_user(&user_id, &user_state.db).await?
            else {
                error!(
                    id,
                    "Failed to validate member id from external id. Bailing in case this was a malicious call",
                );
                return Ok(());
            };

            create_trigger(view_state, &client, user_state, user_id, trusted_member_id).await?;
            Ok(())
        }
        Some(id) if id.starts_with("edit_trigger_") => {
            debug!("Editing trigger");

            let trigger_id = id
                .strip_prefix("edit_trigger_")
                .expect("Failed to parse member id from external id")
                .parse::<i64>()
                .map(models::trigger::Id::new)
                .expect("Failed to parse member id from external id");

            let Some(system) = System::fetch_by_user_id(&user_state.db, &user_id)
                .await
                .ok()
                .flatten()
            else {
                error!(
                    %user_id,
                    "Failed to fetch system id for user id. Bailing in case this was a malicious call"
                );
                return Ok(());
            };

            let Ok(trusted_trigger_id) = trigger_id
                .validate_by_system(system.id, &user_state.db)
                .await
            else {
                error!(
                    "Failed to validate member id from external id {}. Bailing in case this was a malicious call",
                    id
                );
                return Ok(());
            };

            edit_trigger(view_state, &client, user_state, user_id, trusted_trigger_id).await?;
            Ok(())
        }
        Some(id) => {
            error!("receieved unknown external id: {id}");
            Ok(())
        }
    }
}
