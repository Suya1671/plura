use crate::{
    fields, id,
    models::member::{DetectedMember, Member},
};

use super::{
    member::{self},
    trigger::Trigger,
    trust::{Trustability, Trusted},
    user,
};
use error_stack::{Result, ResultExt};
use redact::Secret;
use sqlx::{SqlitePool, prelude::*};
use tracing::debug;

id!(
    /// An ID for a [`System`].
    ///
    /// For an ID to be trusted, it must:
    ///
    /// - Be a valid ID in the database
    /// - Be associated with a valid user
    => System
);

impl Id<Trusted> {
    #[tracing::instrument(skip(db))]
    pub async fn list_triggers(self, db: &SqlitePool) -> Result<Vec<Trigger>, sqlx::Error> {
        Trigger::fetch_by_system_id(self, db).await
    }

    #[tracing::instrument(skip(db))]
    pub async fn change_fronting_member(
        self,
        new_active_member_id: Option<member::Id<Trusted>>,
        db: &SqlitePool,
    ) -> Result<Option<Member>, sqlx::Error> {
        debug!(
            "Changing active member for {} to {:?}",
            self, new_active_member_id
        );

        let mut new_active_member = None;

        if let Some(new_active_member_id) = new_active_member_id {
            new_active_member = Some(
                Member::fetch_by_id(new_active_member_id, db)
                    .await
                    .attach_printable("Failed to fetch member")?,
            );
        }

        fields!(new_active_member = ?&new_active_member);

        sqlx::query!(
            r#"
            UPDATE systems
            SET currently_fronting_member_id = $1
            WHERE id = $2
            "#,
            new_active_member_id,
            self.id
        )
        .execute(db)
        .await
        .attach_printable("Failed to update system active member")?;

        Ok(new_active_member)
    }

    #[tracing::instrument(skip(db))]
    pub async fn currently_fronting_member_id(
        &self,
        db: &SqlitePool,
    ) -> Result<Option<member::Id<Trusted>>, sqlx::Error> {
        sqlx::query!(
            r#"
            SELECT currently_fronting_member_id as "id: member::Id<Trusted>"
            FROM systems
            WHERE id = $1
            "#,
            self.id
        )
        .fetch_one(db)
        .await
        .attach_printable("Failed to fetch system currently fronting member id")
        .map(|row| row.id)
    }

    #[tracing::instrument(skip(db))]
    pub async fn fetch(self, db: &SqlitePool) -> Result<System, sqlx::Error> {
        sqlx::query_as!(
            System,
            r#"
            SELECT
                id as "id: Id<Trusted>",
                owner_id as "owner_id: user::Id<Trusted>",
                currently_fronting_member_id as "currently_fronting_member_id: member::Id<Trusted>",
                auto_switch_on_trigger,
                slack_oauth_token,
                created_at as "created_at: time::PrimitiveDateTime"
            FROM systems
            WHERE id = $1
            "#,
            self.id
        )
        .fetch_one(db)
        .await
        .attach_printable("Failed to fetch system from id")
    }
}

#[derive(Debug, FromRow, PartialEq, Eq, Clone)]
#[sqlx(transparent)]
pub struct SlackOauthToken(Secret<String>);

impl SlackOauthToken {
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl From<String> for SlackOauthToken {
    fn from(value: String) -> Self {
        Self(Secret::new(value))
    }
}

#[derive(FromRow, Debug)]
#[allow(dead_code)]
/// A plural system
///
/// A system has 1 owner and many members.
/// The owner is the slack user who created the system.
/// See [`member::Member`] for more information about members.
///
/// A system may have an active currently fronting member. Any messages sent by the system will be sent by this member.
pub struct System {
    #[sqlx(flatten)]
    /// The unique identifier for the system.
    pub id: Id<Trusted>,
    /// The owner of the system.
    pub owner_id: user::Id<Trusted>,
    /// The currently fronting member, if any
    pub currently_fronting_member_id: Option<member::Id<Trusted>>,
    /// Whether a [`trigger::Trigger`] activation changes the active member to the member the trigger is associated with
    pub auto_switch_on_trigger: bool,
    /// The Slack OAuth token for the system
    pub slack_oauth_token: SlackOauthToken,
    pub created_at: time::PrimitiveDateTime,
}

impl System {
    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_user_id<T>(
        user_id: &user::Id<T>,
        db: &SqlitePool,
    ) -> Result<Option<Self>, sqlx::Error>
    where
        T: Trustability,
    {
        sqlx::query_as!(
            System,
            r#"
            SELECT
                id as "id: Id<Trusted>",
                owner_id as "owner_id: user::Id<Trusted>",
                currently_fronting_member_id as "currently_fronting_member_id: member::Id<Trusted>",
                auto_switch_on_trigger,
                slack_oauth_token,
                created_at as "created_at: time::PrimitiveDateTime"
            FROM
                systems
            WHERE owner_id = $1
            "#,
            // This is safe, as this function effectively checks if the user is trusted before fetching the system
            user_id.id
        )
        .fetch_optional(db)
        .await
        .attach_printable("Error fetching system")
    }

    #[tracing::instrument(skip(db))]
    pub async fn active_member(&self, db: &SqlitePool) -> Result<Option<Member>, sqlx::Error> {
        match self.currently_fronting_member_id {
            Some(id) => Ok(Some(Member::fetch_by_id(id, db).await?)),
            None => Ok(None),
        }
    }

    #[tracing::instrument(skip(db))]
    pub async fn change_fronting_member(
        &mut self,
        new_fronting_member_id: Option<member::Id<Trusted>>,
        db: &SqlitePool,
    ) -> Result<Option<Member>, sqlx::Error> {
        let new_active_member = self
            .id
            .change_fronting_member(new_fronting_member_id, db)
            .await?;

        self.currently_fronting_member_id = new_fronting_member_id;
        Ok(new_active_member)
    }

    pub async fn members(&self, db: &SqlitePool) -> Result<Vec<Member>, sqlx::Error> {
        sqlx::query_as!(
            Member,
            r#"
            SELECT
                id as "id: member::Id<Trusted>",
                system_id as "system_id: Id<Trusted>",
                full_name,
                display_name,
                profile_picture_url,
                title,
                pronouns,
                name_pronunciation,
                name_recording_url,
                enabled,
                created_at as "created_at: time::PrimitiveDateTime"
            FROM
                members
            WHERE system_id = $1
            "#,
            self.id
        )
        .fetch_all(db)
        .await
        .attach_printable("Failed to fetch members")
    }

    pub async fn find_member_by_trigger_rules(
        &self,
        db: &SqlitePool,
        message: &str,
    ) -> Result<Option<DetectedMember>, sqlx::Error> {
        debug!(message, "Finding detected member if there is a match");
        sqlx::query_as!(
            DetectedMember,
            r#"
                SELECT
                    members.id as "id: member::Id<Trusted>",
                    display_name,
                    profile_picture_url,
                    triggers.text as trigger_text,
                    triggers.typ
                FROM
                    members
                JOIN
                    triggers ON members.id = triggers.member_id
                WHERE
                    -- See trigger.rs file for all types and names
                    members.enabled = TRUE AND
                    ((triggers.typ = 0 AND $1 LIKE '%' || triggers.text) OR
                    (triggers.typ = 1 AND $1 LIKE triggers.text || '%'))
            "#,
            message
        )
        .fetch_optional(db)
        .await
        .attach_printable("Failed to fetch triggered member")
    }
}
