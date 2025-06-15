use crate::{
    id,
    models::member::{Member, TriggeredMember},
};

use super::{
    Trustability, Trusted,
    member::{self},
    trigger::Trigger,
    user,
};
use redact::Secret;
use sqlx::{SqlitePool, prelude::*};
use tracing::debug;

id!(
    /// For an ID to be trusted, it must
    ///
    /// - Be a valid ID in the database
    /// - Be associated with a valid user
    => System
);

impl Id<Trusted> {
    pub async fn list_triggers(self, db: &SqlitePool) -> Result<Vec<Trigger>, sqlx::Error> {
        Trigger::fetch_by_system_id(db, self).await
    }

    pub async fn rename(self, new_name: &str, db: &SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE systems SET name = ? WHERE id = ?",
            new_name,
            self.id
        )
        .execute(db)
        .await?;

        Ok(())
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
pub struct System {
    #[sqlx(flatten)]
    pub id: Id<Trusted>,
    pub owner_id: user::Id<Trusted>,
    pub active_member_id: Option<member::Id<Trusted>>,
    pub trigger_changes_active_member: bool,
    pub name: String,
    pub slack_oauth_token: SlackOauthToken,
    pub created_at: time::PrimitiveDateTime,
}

#[derive(Debug, thiserror::Error, displaydoc::Display)]
/// Error while changing the active member
pub enum ChangeActiveMemberError {
    /// Error while calling the database
    Sqlx(#[from] sqlx::Error),
    /// The member is not part of the system
    MemberNotFound,
}

impl System {
    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_user_id<T>(
        db: &SqlitePool,
        user_id: &user::Id<T>,
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
            active_member_id as "active_member_id: member::Id<Trusted>",
            trigger_changes_active_member,
            slack_oauth_token,
            name,
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
    }

    pub async fn active_member(&self, db: &SqlitePool) -> Result<Option<Member>, sqlx::Error> {
        match self.active_member_id {
            Some(id) => Member::fetch_by_id(id, db).await,
            None => Ok(None),
        }
    }

    #[tracing::instrument(skip(db))]
    pub async fn change_active_member(
        &mut self,
        new_active_member_id: Option<member::Id<Trusted>>,
        db: &SqlitePool,
    ) -> Result<Option<Member>, ChangeActiveMemberError> {
        debug!(
            "Changing active member for {} to {:?}",
            self.id, new_active_member_id
        );
        let mut new_active_member = None;

        if let Some(new_active_member_id) = new_active_member_id {
            let Some(member) = Member::fetch_by_id(new_active_member_id, db).await? else {
                return Err(ChangeActiveMemberError::MemberNotFound);
            };

            new_active_member = Some(member);
        }

        sqlx::query!(
            r#"
            UPDATE systems
            SET active_member_id = $1
            WHERE id = $2
            "#,
            new_active_member_id,
            self.id
        )
        .execute(db)
        .await?;

        self.active_member_id = new_active_member_id;
        Ok(new_active_member)
    }

    pub async fn get_members(&self, db: &SqlitePool) -> Result<Vec<Member>, sqlx::Error> {
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
                created_at as "created_at: time::PrimitiveDateTime"
            FROM
                members
            WHERE system_id = $1
            "#,
            self.id
        )
        .fetch_all(db)
        .await
    }

    pub async fn fetch_triggered_member(
        &self,
        db: &SqlitePool,
        message: &str,
    ) -> Result<Option<TriggeredMember>, sqlx::Error> {
        sqlx::query_as!(
            TriggeredMember,
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
                    (triggers.typ = 0 AND ?1 LIKE triggers.text || '%') OR
                    (triggers.typ = 1 AND ?1 LIKE '%' || triggers.text)
            "#,
            message
        )
        .fetch_optional(db)
        .await
    }
}
