use error_stack::{Result, ResultExt, bail};
use slack_morphism::prelude::*;

use crate::{
    BOT_TOKEN,
    models::{
        Trusted, member,
        system::System,
        trigger,
        user::{self, State},
    },
};

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum Error {
    /// Error while calling the database
    Sqlx,
    /// Error while calling the Slack API
    Slack,
    /// No system found for the user
    NoSystem,
}

pub async fn create_trigger(
    view_state: SlackViewState,
    client: &SlackHyperClient,
    user_state: &State,
    user_id: user::Id<Trusted>,
    member_id: member::Id<Trusted>,
) -> Result<(), Error> {
    let data = trigger::View::from(view_state);

    let Some(system_id) = System::fetch_by_user_id(&user_state.db, &user_id)
        .await
        .attach_printable("Error checking if system exists")
        .change_context(Error::Sqlx)?
        .map(|system| system.id)
    else {
        bail!(Error::NoSystem);
    };

    let _id = data
        .add(system_id, member_id, &user_state.db)
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
            SlackMessageContent::new().with_text("Successfully added trigger!".into()),
        ))
        .await
        .change_context(Error::Slack)?;

    Ok(())
}

pub async fn edit_trigger(
    view_state: SlackViewState,
    client: &SlackHyperClient,
    user_state: &State,
    user_id: user::Id<Trusted>,
    trigger_id: trigger::Id<Trusted>,
) -> Result<(), Error> {
    let trigger_view = trigger::View::from(view_state);

    trigger_view
        .update(trigger_id, &user_state.db)
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
            SlackMessageContent::new().with_text("Successfully edited trigger!".into()),
        ))
        .await
        .change_context(Error::Slack)?;

    Ok(())
}
