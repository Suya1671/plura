use std::str::FromStr;

use crate::id;

use super::{Trusted, Untrusted, member, system};
use error_stack::{Result, ResultExt};
use sqlx::{SqlitePool, prelude::*, sqlite::SqliteQueryResult};

id!(
    /// For an ID to be trusted, it must
    ///
    /// - Be a valid ID in the database
    /// - Be associated with a valid member or system
    => Trigger
);

impl Id<Untrusted> {
    #[tracing::instrument(skip(db))]
    pub async fn validate_by_system(
        self,
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Id<Trusted>, sqlx::Error> {
        sqlx::query!(
            "SELECT
                id as 'id: Id<Trusted>'
            FROM triggers
            WHERE id = $1 AND system_id = $2",
            self.id,
            system_id.id
        )
        .fetch_one(db)
        .await
        .map(|record| record.id)
        .attach_printable("Error validating trigger")
    }
}

impl Id<Trusted> {
    #[tracing::instrument(skip(db))]
    pub async fn delete(self, db: &SqlitePool) -> Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!(
            r#"
                DELETE FROM triggers
                WHERE id = $1
            "#,
            self.id
        )
        .execute(db)
        .await
        .attach_printable("Error deleting trigger")
    }

    #[tracing::instrument(skip(db))]
    pub async fn update(
        self,
        typ: Option<Type>,
        content: Option<String>,
        db: &SqlitePool,
    ) -> error_stack::Result<Self, sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE triggers
            SET
                typ = coalesce($2, typ),
                text = coalesce($3, text)
            WHERE id = $1
            RETURNING
                id as "id: Id<Trusted>"
            "#,
            self,
            typ,
            content
        )
        .fetch_one(db)
        .await
        .attach_printable("Failed to update trigger")
        .map(|record| record.id)
    }
}

#[derive(Debug, sqlx::Type, displaydoc::Display, PartialEq, Eq, clap::ValueEnum, Clone, Copy)]
#[repr(i64)]
pub enum Type {
    /// Suffix
    Suffix = 0,
    /// Prefix
    Prefix = 1,
}

impl From<i64> for Type {
    fn from(value: i64) -> Self {
        match value {
            0 => Self::Suffix,
            1 => Self::Prefix,
            _ => unreachable!(
                "Invalid type value. This means the database and rust struct are out of sync"
            ),
        }
    }
}

#[derive(Debug, displaydoc::Display)]
/// Unknown type
pub struct UnknownType(String);

impl FromStr for Type {
    type Err = UnknownType;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "suffix" => Ok(Self::Suffix),
            "prefix" => Ok(Self::Prefix),
            _ => Err(UnknownType(s.to_string())),
        }
    }
}

#[derive(FromRow, Debug)]
#[allow(dead_code)]
pub struct Trigger {
    pub id: Id<Trusted>,
    pub member_id: member::Id<Trusted>,
    pub system_id: system::Id<Trusted>,
    pub text: String,
    pub typ: Type,
}

impl Trigger {
    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_system_id(
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Trigger,
            r#"
                SELECT
                    id as "id: Id<Trusted>",
                    member_id as "member_id: member::Id<Trusted>",
                    system_id as "system_id: system::Id<Trusted>",
                    text,
                    typ
                FROM
                    triggers
                WHERE
                   system_id = $1
                "#,
            system_id
        )
        .fetch_all(db)
        .await
        .attach_printable("Error fetching triggers")
    }

    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_member_id(
        member_id: member::Id<Trusted>,
        db: &SqlitePool,
    ) -> error_stack::Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Trigger,
            r#"
            SELECT
                id as "id: Id<Trusted>",
                member_id as "member_id: member::Id<Trusted>",
                system_id as "system_id: system::Id<Trusted>",
                text,
                typ
            FROM
                triggers
            WHERE member_id = $1
            "#,
            member_id,
        )
        .fetch_all(db)
        .await
        .attach_printable("Error fetching triggers")
    }

    #[tracing::instrument(skip(db))]
    pub async fn insert(
        member_id: member::Id<Trusted>,
        system_id: system::Id<Trusted>,
        typ: Type,
        content: String,
        db: &SqlitePool,
    ) -> error_stack::Result<Self, sqlx::Error> {
        sqlx::query_as!(
            Self,
            r#"
            INSERT INTO triggers (member_id, system_id, typ, text)
            VALUES ($1, $2, $3, $4)
            RETURNING
                id as "id: Id<Trusted>",
                member_id as "member_id: member::Id<Trusted>",
                system_id as "system_id: system::Id<Trusted>",
                typ,
                text
            "#,
            member_id,
            system_id,
            typ,
            content
        )
        .fetch_one(db)
        .await
        .attach_printable("Failed to insert trigger into database")
    }
}
