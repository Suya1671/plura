use crate::id;

use super::{
    member, system,
    trust::{Trusted, Untrusted},
};
use error_stack::{Result, ResultExt};
use sqlx::{SqlitePool, prelude::*, sqlite::SqliteQueryResult};

id!(
    /// For an ID to be trusted, it must
    ///
    /// - Be a valid ID in the database
    /// - Be associated with a valid member and system
    => Alias
);

impl Id<Untrusted> {
    #[tracing::instrument(skip(db))]
    pub async fn validate_by_system(
        self,
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Option<Id<Trusted>>, sqlx::Error> {
        sqlx::query!(
            "SELECT
                id as 'id: Id<Trusted>'
            FROM aliases
            WHERE id = $1 AND system_id = $2",
            self.id,
            system_id.id
        )
        .fetch_optional(db)
        .await
        .map(|res| res.map(|res| res.id))
        .attach_printable("Failed to fetch alias id from database")
    }
}

impl Id<Trusted> {
    #[tracing::instrument(skip(db))]
    pub async fn delete(self, db: &SqlitePool) -> Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!(
            r#"
                DELETE FROM aliases
                WHERE id = $1
            "#,
            self.id
        )
        .execute(db)
        .await
        .attach_printable("Failed to delete alias from database")
    }

    #[tracing::instrument(skip(db))]
    pub async fn change_alias(
        self,
        new_alias: String,
        db: &SqlitePool,
    ) -> Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!(
            r#"
                UPDATE aliases
                SET alias = $2
                WHERE id = $1
            "#,
            self.id,
            new_alias
        )
        .execute(db)
        .await
        .attach_printable("Failed to change alias in database")
    }
}

#[derive(FromRow, Debug)]
#[allow(dead_code)]
pub struct Alias {
    pub id: Id<Trusted>,
    pub member_id: member::Id<Trusted>,
    pub system_id: system::Id<Trusted>,
    #[allow(clippy::struct_field_names)]
    pub alias: String,
}

impl Alias {
    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_system_id(
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Alias,
            r#"
                SELECT
                    id as "id: Id<Trusted>",
                    member_id as "member_id: member::Id<Trusted>",
                    system_id as "system_id: system::Id<Trusted>",
                    alias
                FROM
                    aliases
                WHERE
                   system_id = $1
                "#,
            system_id
        )
        .fetch_all(db)
        .await
        .attach_printable("Failed to fetch aliases from database")
    }

    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_member_id(
        member_id: member::Id<Trusted>,
        db: &SqlitePool,
    ) -> error_stack::Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Self,
            r#"
            SELECT
                id as "id: Id<Trusted>",
                member_id as "member_id: member::Id<Trusted>",
                system_id as "system_id: system::Id<Trusted>",
                alias
            FROM
                aliases
            WHERE member_id = $1
            "#,
            member_id,
        )
        .fetch_all(db)
        .await
        .attach_printable("Failed to fetch aliases from database")
    }

    #[tracing::instrument(skip(db))]
    pub async fn insert(
        member_id: member::Id<Trusted>,
        system_id: system::Id<Trusted>,
        alias: String,
        db: &SqlitePool,
    ) -> error_stack::Result<Self, sqlx::Error> {
        sqlx::query_as!(
            Self,
            r#"
            INSERT INTO aliases (member_id, system_id, alias)
            VALUES ($1, $2, $3)
            RETURNING
                id as "id: Id<Trusted>",
                member_id as "member_id: member::Id<Trusted>",
                system_id as "system_id: system::Id<Trusted>",
                alias
            "#,
            member_id,
            system_id,
            alias,
        )
        .fetch_one(db)
        .await
        .attach_printable("Failed to insert alias into database")
    }
}
