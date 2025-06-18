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
}

#[derive(Debug)]
pub struct View {
    pub alias: String,
}

impl Default for View {
    fn default() -> Self {
        Self {
            alias: String::new(),
        }
    }
}

impl View {
    pub fn create_blocks(self) -> Vec<SlackBlock> {
        slack_blocks!(
            some_into(
                SlackHeaderBlock::new("Alias settings".into())
                    .with_block_id("alias_settings".into())
            ),
            some_into(
                SlackInputBlock::new(
                    "Alias".into(),
                    SlackBlockPlainTextInputElement::new("alias".into())
                        .with_initial_value(self.alias)
                        .into(),
                )
                .with_optional(false)
            )
        )
    }

    /// Add a member alias to the database
    ///
    /// Returns the id of the new alias
    pub async fn add(
        &self,
        system_id: system::Id<Trusted>,
        member_id: member::Id<Trusted>,
        db_pool: &SqlitePool,
    ) -> Result<Id<Trusted>, sqlx::Error> {
        debug!(
            "Adding alias for {} (Member ID {}) to database",
            system_id, member_id
        );

        sqlx::query!(
            r#"
            INSERT INTO aliases (system_id, member_id, alias)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
            system_id.id,
            member_id.id,
            self.alias,
        )
        .fetch_one(db_pool)
        .await
        .attach_printable("Error adding alias to database")
        .map(|row| Id {
            id: row.id,
            trusted: std::marker::PhantomData,
        })
    }

    /// Update a member alias in the database to match this view
    pub async fn update(
        &self,
        alias_id: Id<Trusted>,
        db: &SqlitePool,
    ) -> error_stack::Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE aliases
            SET alias = $1
            WHERE id = $2
            "#,
            self.alias,
            alias_id.id,
        )
        .execute(db)
        .await
        .attach_printable("Error updating member alias in database")
    }

    pub fn create_add_view(self, member_id: member::Id<Trusted>) -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Add a new member alias".into(), self.create_blocks())
                .with_submit("Add".into())
                .with_external_id(format!("create_member_alias_{}", member_id.id)),
        )
    }

    pub fn create_edit_view(self, alias_id: Id<Trusted>) -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Edit member alias".into(), self.create_blocks())
                .with_submit("Save".into())
                .with_external_id(format!("edit_member_alias_{}", alias_id.id)),
        )
    }
}

impl From<SlackViewState> for View {
    fn from(value: SlackViewState) -> Self {
        let mut view = Self::default();
        for (_id, values) in value.values {
            for (id, content) in values {
                match &*id.0 {
                    "alias" => {
                        if let Some(text) = content.value {
                            view.alias = text;
                        }
                    }
                    other => {
                        warn!("Unknown field in view when parsing a alias::View: {other}");
                    }
                }
            }
        }

        view
    }
}

impl From<Alias> for View {
    fn from(alias: Alias) -> Self {
        Self { alias: alias.alias }
    }
}
