use std::sync::Arc;

use error_stack::{Result, ResultExt, report};
use futures::TryStreamExt;
use slack_morphism::prelude::*;
use tracing::{debug, info, trace};

use crate::{
    BOT_TOKEN, fetch_member, fetch_system, fields,
    models::{
        self,
        member::{self, MemberRef, View},
        trust::Untrusted,
        user,
    },
};

#[derive(clap::Subcommand, Debug)]
#[clap(verbatim_doc_comment)]
/// A member is an "alter" who is part of your system. This manages member profiles.
///
/// The main feature of this bot is that it allows you to treat each member profile like a Slack account.
/// You can send messages, define information about them, etc. Without having to make new Slack accounts for each member and manage them.
///
/// This does come with limitations:
/// - Each member is not a Slack account. It's simply this bot sending something under the name of the member.
/// - This also means members have a custom
///
/// Also see:
/// - /triggers to manage member triggers (Custom prefixes/suffixes that will automatically message under a specific member profile) \n
/// - /aliases to manage member aliases (Custom names that can be used to refer to the member in commands)
pub enum Member {
    /// Adds a new member to your system. Expect a popup to fill in the member info!
    Add,
    /// Disables/Deletes a member from your system.
    ///
    /// This doesn't actually "delete" the member entirely, nor does it delete messages sent by this member.
    /// Rather, the member is disabled and cannot be accessed. This is for moderation purposes.
    /// If you wish for the member to be re-enabled, you can use the `/members enable` command.
    ///
    /// Disabling a member also prevents them from being accessed via their aliases or triggers.
    Disable {
        /// The member to delete
        member: MemberRef,
    },
    /// Enables a member from your system.
    ///
    /// This will re-enable the member and allow them to be accessed again.
    Enable {
        /// The member to enable
        member: member::Id<Untrusted>,
    },
    /// Gets info about a member
    ///
    /// This will display information about the member, including their name, pronouns, and other details.
    Info {
        /// The member to get info about. You must use the member's ID, which you can get from /members list.
        member_id: MemberRef,
    },
    /// Lists all members in a system
    ///
    /// This will contain basic information about each member.
    /// For more detailed information, use the `/members info` command.
    List {
        /// The system to list members from. If left blank, defaults to your system.
        system: Option<String>,
    },
    /// Edits a member's info
    ///
    ///  Expect a popup to edit the info!
    Edit {
        /// The member to edit.
        member_id: MemberRef,
    },
    /// Switch to a different member
    ///
    /// You can switch to a different member by providing their ID or username.
    /// Alternatively, you can use `/members switch --base` to revert to your base account,
    /// and the bot will not rewrite messages under a member profile.
    #[group(required = true)]
    Switch {
        /// The member to switch to.
        #[clap(group = "member")]
        member_id: Option<MemberRef>,
        /// Don't switch to another member, just message with the base account
        #[clap(long, short, action, group = "member", alias = "none")]
        base: bool,
    },
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum CommandError {
    /// Error while calling the Slack API
    SlackApi,
    /// Error while calling the database
    Sqlx,
}

impl Member {
    #[tracing::instrument(skip_all)]
    pub async fn run(
        self,
        event: SlackCommandEvent,
        client: Arc<SlackHyperClient>,
        state: SlackClientEventsUserState,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running members command");
        match self {
            Self::Add => {
                let token = &BOT_TOKEN;
                let session = client.open_session(token);
                Self::create_member(event, session).await
            }
            Self::Disable { member } => Self::disable(event, &state, member).await,
            Self::Enable { member } => Self::enable(event, &state, member).await,
            Self::Info { member_id } => Self::member_info(event, &state, member_id).await,
            Self::Edit { member_id } => {
                Self::edit_member(event, client.open_session(&BOT_TOKEN), &state, member_id).await
            }
            Self::List { system } => Self::list_members(event, state, system).await,
            Self::Switch { member_id, base } => {
                Self::switch_member(event, state, member_id, base).await
            }
        }
    }

    #[tracing::instrument(skip(event, state), fields(system_id))]
    async fn switch_member(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        member_ref: Option<MemberRef>,
        base: bool,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Switching member");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        let new_active_member_id = if base {
            None
        } else {
            debug!(requested_member_id = ?&member_ref, "Validating member ID");
            fetch_member!(member_ref.as_ref().unwrap(), user_state, system_id => member_id);

            if !member_id
                .enabled(&user_state.db)
                .await
                .change_context(CommandError::Sqlx)?
            {
                debug!("Member is disabled");

                return Ok(SlackCommandEventResponse::new(
                    SlackMessageContent::new()
                        .with_text("The member you're trying to switch to is disabled! Either re-enable them or choose another member.".into()),
                ));
            }

            Some(member_id)
        };

        debug!(target_member_id = ?new_active_member_id, "Changing active member");

        let new_member = system_id
            .change_fronting_member(new_active_member_id, &user_state.db)
            .await;

        let response = match new_member {
            Ok(Some(member)) => {
                info!(member_name = %member.full_name, member_id = %member.id, "Successfully switched to member");
                format!("Switch to member {}", member.full_name)
            }
            Ok(None) => {
                info!("Successfully switched to base account");
                "Switched to base account".into()
            }
            Err(e) => return Err(e.change_context(CommandError::Sqlx)),
        };

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text(response),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(user_id, system_id))]
    async fn list_members(
        event: SlackCommandEvent,
        state: SlackClientEventsUserState,
        system: Option<String>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Listing all members");
        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        // If the input exists, parse it into a user ID
        // If it doesn't exist, use the user ID of the event.
        // If the user ID is invalid, return an error.
        // There's probably a better way to write this behaviour but I'm not sure how.
        let Some((user_id, is_author)) = system.map_or_else(
            || Some((user::Id::new(event.user_id), true)),
            |u| user::parse_slack_user_id(&u).map(|id| (id, false)),
        ) else {
            debug!("Invalid user ID provided in system parameter");
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Invalid user ID".into()),
            ));
        };

        fields!(user_id = %user_id.clone());

        let Some(system) = models::System::fetch_by_user_id(&user_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        else {
            debug!(target_user_id = %user_id, is_self = is_author, "Target user has no system");
            return if is_author {
                Ok(SlackCommandEventResponse::new(
                    SlackMessageContent::new().with_text("You don't have a system yet!".into()),
                ))
            } else {
                Ok(SlackCommandEventResponse::new(
                    SlackMessageContent::new().with_text("This user doesn't have a system!".into()),
                ))
            };
        };

        fields!(system_id = %system.id);

        let member_blocks = sqlx::query!(
            "
                SELECT
                    members.id,
                    display_name,
                    full_name,
                    enabled,
                    GROUP_CONCAT(aliases.alias, ', ') as aliases
                FROM
                    members
                JOIN
                    aliases ON members.id = aliases.member_id
                WHERE
                    members.system_id = $1
                GROUP BY members.id
            ",
            system.id
        )
        .fetch(&user_state.db)
        .map_ok(|member| {
            let fields = [
                Some(md!("*Member ID*: {}", member.id)),
                Some(md!("*Display Name*: {}", member.display_name)),
                Some(md!("*Aliases: {}", member.aliases)),
                Some(md!("*Disabled*")).filter(|_| !member.enabled),
            ]
            .into_iter()
            .flatten()
            .collect();

            SlackSectionBlock::new()
                .with_text(md!("*{}*", member.full_name))
                .with_fields(fields)
        })
        .map_ok(Into::into)
        .map_err(|err| report!(err).change_context(CommandError::Sqlx))
        .try_collect()
        .await?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(member_blocks),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(user_id = %event.user_id, system_id, member_id))]
    async fn disable(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member_ref: MemberRef,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member disable command");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        fetch_member!(member_ref, user_state, system_id => member_id);

        if !member_id
            .enabled(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Member is already disabled".into()),
            ));
        }

        let system_fronting_member_id = system_id
            .currently_fronting_member_id(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        if system_fronting_member_id.is_some_and(|id| id == member_id) {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Cannot disable the currently fronting member. You can use `/members switch` to switch to another member.".into()),
            ));
        }

        member_id
            .set_enabled(false, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Member disabled".into()),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(user_id = %event.user_id, system_id, member_id))]
    async fn enable(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member: member::Id<Untrusted>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member enable command");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        fetch_member!(member, user_state, system_id => member_id);

        if member_id
            .enabled(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text("Member is already enabled".into()),
            ));
        }

        member_id
            .set_enabled(true, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("Member enabled".into()),
        ))
    }

    #[tracing::instrument(skip(event, state), fields(user_id = %event.user_id, system_id, member_id))]
    async fn member_info(
        event: SlackCommandEvent,
        state: &SlackClientEventsUserState,
        member_ref: MemberRef,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member info command");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        fetch_member!(member_ref, user_state, system_id => member_id);

        let member = models::Member::fetch_by_id(member_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        debug!("Member found");

        if !member.enabled {
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new().with_text(format!(
                    "Member {} is not enabled. You can use `/member enable {}` to enable them.",
                    member.full_name, member.id
                )),
            ));
        }

        let system_fronting_member_id = system_id
            .currently_fronting_member_id(&user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        let blocks = slack_blocks![
            some_into(SlackHeaderBlock::new(member.full_name.into())),
            some_into(SlackDividerBlock::new()),
            some_into(
                SlackSectionBlock::new()
                    .with_text(md!(
                        "*{}*\n{}{}",
                        member.display_name,
                        member.pronouns.unwrap_or_default(),
                        member
                            .name_pronunciation
                            .map(|pronunciation| format!(" - {pronunciation}"))
                            .unwrap_or_default()
                    ))
                    .opt_accessory(member.profile_picture_url.and_then(|url| Some(
                        SlackSectionBlockElement::Image(SlackBlockImageElement::new(
                            url.parse().ok()?,
                            "Profile picture".into()
                        ))
                    )))
            ),
            optionally_into(system_fronting_member_id.is_some_and(|id| id == member.id) => SlackSectionBlock::new().with_text(md!("*Fronting*")))
            // TO-DO: fields
        ];

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_blocks(blocks),
        ))
    }

    #[tracing::instrument(skip(event, session), fields(view_id))]
    async fn create_member(
        event: SlackCommandEvent,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member creation command");
        let view = View::create_add_view();

        let view = session
            .views_open(&SlackApiViewsOpenRequest::new(event.trigger_id, view))
            .await
            .attach_printable("Error opening view")
            .change_context(CommandError::SlackApi)?;

        info!(view_id = %view.view.state_params.id, "Successfully opened member creation view");

        Ok(SlackCommandEventResponse::new(
            SlackMessageContent::new().with_text("View opened!".into()),
        ))
    }

    #[tracing::instrument(skip(event, session, state), fields(user_id = %event.user_id, trigger_id = %event.trigger_id))]
    async fn edit_member(
        event: SlackCommandEvent,
        session: SlackClientSession<'_, SlackClientHyperHttpsConnector>,
        state: &SlackClientEventsUserState,
        member_ref: MemberRef,
    ) -> Result<SlackCommandEventResponse, CommandError> {
        trace!("Running member edit command");

        let states = state.read().await;
        let user_state = states.get_user_state::<user::State>().unwrap();

        fetch_system!(event, user_state => system_id);

        fetch_member!(member_ref, user_state, system_id => member_id);

        let member = models::Member::fetch_by_id(member_id, &user_state.db)
            .await
            .change_context(CommandError::Sqlx)?;

        let view = member::View::from(member).create_edit_view(member_id);

        let view = session
            .views_open(&SlackApiViewsOpenRequest::new(
                event.trigger_id.clone(),
                view,
            ))
            .await
            .attach_printable("Error opening view")
            .change_context(CommandError::SlackApi)?;

        info!(view_id = %view.view.state_params.id, member_id = %member_id, "Successfully opened member edit view");

        Ok(SlackCommandEventResponse::new(SlackMessageContent::new()))
    }
}

#[macro_export]
/// Fetches the member ID associated with the
/// Also attaches the member ID to context
///
/// Else, returns early with a warning message
macro_rules! fetch_member {
    ($member_ref:expr, $user_state:expr, $system_id:expr => $member_var_name:ident) => {
        let Some($member_var_name) = $member_ref
            .validate_by_system($system_id, &$user_state.db)
            .await
            .change_context(CommandError::Sqlx)?
        else {
            use slack_morphism::prelude::*;
            ::tracing::debug!("User does not have a member with alias {:?} that is associated with the system", $member_ref);
            return Ok(SlackCommandEventResponse::new(
                SlackMessageContent::new()
                    .with_text("The member does not exist! Make sure you spelt the alias correctly or used the correct ID.".to_string()),
            ));
        };

        $crate::fields!(member_id = %$member_var_name);
        ::tracing::debug!("Fetched member");
    };
}
