use crate::id;

use super::{Trusted, member};
use error_stack::{Result, ResultExt};
use slack_morphism::SlackTs;
use sqlx::{SqlitePool, prelude::*, sqlite::SqliteQueryResult};

id!(
    /// You cannot create a message id, as it is internal generated-only.
    ///
    /// For an ID to be valid (trusted), it must
    ///
    /// - Be associated with a valid user (constrained at database level; no validation needed)
    /// - Have a message ID that exists in Slack and the bot can access (there shouldn't be a message without a message ID; no validation needed)
    => Message
);

#[derive(FromRow, Debug)]
#[allow(dead_code)]
pub struct MessageLog {
    pub id: Id<Trusted>,
    pub member_id: member::Id<Trusted>,
    #[sqlx(try_from = "String")]
    pub message_id: SlackTs,
}

impl MessageLog {
    /// Deletes a message log by the message ID.
    pub async fn delete_by_message_id(
        message_id: String,
        db: &SqlitePool,
    ) -> Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!(
            r#"
                DELETE FROM message_logs
                WHERE message_id = $1
            "#,
            message_id
        )
        .execute(db)
        .await
        .attach_printable("Failed to delete message log")
    }

    /// Fetches a message log by the slack message ID.
    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_message_id(
        id: String,
        db: &SqlitePool,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            MessageLog,
            r#"
            SELECT
                id as "id: Id<Trusted>",
                member_id as "member_id: member::Id<Trusted>",
                message_id
            FROM
                message_logs
            WHERE message_id = $1
            "#,
            id,
        )
        .fetch_optional(db)
        .await
        .attach_printable("Failed to fetch message log")
    }

    /// Fetches all message logs by the member ID.
    #[tracing::instrument(skip(db))]
    pub async fn fetch_all_by_member_id(
        db: &SqlitePool,
        member_id: member::Id<Trusted>,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            MessageLog,
            r#"
                SELECT
                    id as "id: Id<Trusted>",
                    member_id as "member_id: member::Id<Trusted>",
                    message_id
                FROM
                    message_logs
                WHERE
                   member_id = $1
                "#,
            member_id
        )
        .fetch_all(db)
        .await
        .attach_printable("Failed to fetch message logs")
    }

    pub async fn insert(
        member_id: member::Id<Trusted>,
        message_id: SlackTs,
        db: &SqlitePool,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query_as!(
            MessageLog,
            r#"
                INSERT INTO message_logs (member_id, message_id)
                VALUES ($1, $2)
                RETURNING
                    id as "id: Id<Trusted>",
                    member_id as "member_id: member::Id<Trusted>",
                    message_id
            "#,
            member_id,
            message_id.0
        )
        .fetch_one(db)
        .await
        .attach_printable("Failed to insert message log")
    }
}
