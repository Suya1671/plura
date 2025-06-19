use crate::id;

use super::{Trustability, Trusted, Untrusted, member, system};
use error_stack::{Result, ResultExt};
use slack_morphism::prelude::*;
use sqlx::{SqlitePool, prelude::*, sqlite::SqliteQueryResult};
use tracing::{debug, warn};

id!(
    /// For an ID to be trusted, it must
    ///
    /// - Be a valid ID in the database
    /// - Be associated with a valid member and system
    => Alias
);

impl Id<Untrusted> {
    pub const fn new(id: i64) -> Self {
        Self {
            id,
            trusted: std::marker::PhantomData,
        }
    }

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
    pub async fn delete(self, db_pool: &SqlitePool) -> Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!(
            r#"
                DELETE FROM aliases
                WHERE id = $1
            "#,
            self.id
        )
        .execute(db_pool)
        .await
        .attach_printable("Failed to delete alias from database")
    }

    pub async fn change_alias(
        self,
        db_pool: &SqlitePool,
        new_alias: String,
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
        .execute(db_pool)
        .await
        .attach_printable("Failed to change alias in database")
    }
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum Error {
    /// Error while calling the database
    Sqlx,
}

#[derive(FromRow, Debug)]
#[allow(dead_code)]
pub struct Alias {
    pub id: Id<Trusted>,
    pub member_id: member::Id<Trusted>,
    pub system_id: system::Id<Trusted>,
    pub alias: String,
}

impl Alias {
    pub async fn fetch_by_id<T>(id: Id<T>, db: &SqlitePool) -> Result<Option<Self>, sqlx::Error>
    where
        T: Trustability,
    {
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
            WHERE id = $1
            "#,
            id.id,
        )
        .fetch_optional(db)
        .await
        .attach_printable("Failed to fetch alias from database")
    }

    pub async fn fetch_by_system_id(
        db: &SqlitePool,
        system_id: system::Id<Trusted>,
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

    pub async fn fetch_by_member_id(
        db: &SqlitePool,
        member_id: member::Id<Trusted>,
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

    pub async fn insert(
        db: &SqlitePool,
        member_id: member::Id<Trusted>,
        system_id: system::Id<Trusted>,
        alias: String,
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
