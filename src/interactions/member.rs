use error_stack::{Result, ResultExt, bail};
use slack_morphism::prelude::*;
use tracing::trace;

use crate::{
    BOT_TOKEN, fields,
    models::{
        Trusted, member,
        system::System,
        user::{self, State},
    },
};

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum Error {
    /// Error while calling the database
    Sqlx,
    /// Error while calling the Slack API
    Slack,
    /// Unable to parse view
    ParsingView,
    /// No system found for the user
    NoSystem,
}

#[tracing::instrument(skip(view_state, client, user_state), fields(system_id))]
pub async fn create_member(
    view_state: SlackViewState,
    client: &SlackHyperClient,
    user_state: &State,
    user_id: user::Id<Trusted>,
) -> Result<(), Error> {
    trace!("Creating member");
    let data = member::View::try_from(view_state).change_context(Error::ParsingView)?;

    let Some(system_id) = System::fetch_by_user_id(&user_state.db, &user_id)
        .await
        .attach_printable("Error checking if system exists")
        .change_context(Error::Sqlx)?
        .map(|system| system.id)
    else {
        bail!(Error::NoSystem);
    };

    fields!(system_id = %system_id);

    let id = data
        .add(system_id, &user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    let session = client.open_session(&BOT_TOKEN);
    let user: SlackUserId = user_id.into();

    let conversation = session
        .conversations_open(&SlackApiConversationsOpenRequest::new().with_users(vec![user.clone()]))
        .await
        .change_context(Error::Slack)?
        .channel;

    session
        .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
            conversation.id,
            user,
            SlackMessageContent::new().with_text(format!(
                "Successfully added {}! Their ID is {}",
                data.display_name, id
            )),
        ))
        .await
        .change_context(Error::Slack)?;

    Ok(())
}

#[tracing::instrument(skip(view_state, client, user_state))]
pub async fn edit_member(
    view_state: SlackViewState,
    client: &SlackHyperClient,
    user_state: &State,
    user_id: user::Id<Trusted>,
    member_id: member::Id<Trusted>,
) -> Result<(), Error> {
    trace!("Editing member");
    let data = member::View::try_from(view_state).change_context(Error::ParsingView)?;

    data.update(member_id, &user_state.db)
        .await
        .change_context(Error::Sqlx)?;

    let session = client.open_session(&BOT_TOKEN);
    let user: SlackUserId = user_id.into();

    let conversation = session
        .conversations_open(&SlackApiConversationsOpenRequest::new().with_users(vec![user.clone()]))
        .await
        .change_context(Error::Slack)?
        .channel;

    session
        .chat_post_ephemeral(&SlackApiChatPostEphemeralRequest::new(
            conversation.id,
            user,
            SlackMessageContent::new().with_text(format!(
                "Successfully edited {} (ID {})",
                data.display_name, member_id
            )),
        ))
        .await
        .change_context(Error::Slack)?;

    Ok(())
}
