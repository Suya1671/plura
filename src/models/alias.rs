use crate::id;

use super::{Trustability, Trusted, Untrusted, member, system};
use error_stack::ResultExt;
use slack_morphism::prelude::*;
use sqlx::{SqlitePool, prelude::*};
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
    ) -> Result<Id<Trusted>, Self> {
        let exists = sqlx::query!(
            "SELECT EXISTS(SELECT 1 FROM aliases WHERE id = $1 AND system_id = $2) AS 'exists: bool'",
            self.id,
            system_id.id
        )
        .fetch_one(db)
        .await
        .ok()
        .is_some_and(|record| record.exists);

        if exists {
            Ok(Id {
                id: self.id,
                trusted: std::marker::PhantomData,
            })
        } else {
            Err(self)
        }
    }
}

impl Id<Trusted> {
    pub async fn delete(self, db_pool: &SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
                DELETE FROM aliases
                WHERE id = $1
            "#,
            self.id
        )
        .execute(db_pool)
        .await
        .map(|_| ())
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
    }

    pub async fn fetch_by_member_id(
        db: &SqlitePool,
        member_id: member::Id<Trusted>,
    ) -> error_stack::Result<Vec<Self>, Error> {
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
        .change_context(Error::Sqlx)
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
    ) -> error_stack::Result<Id<Trusted>, Error> {
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
        .change_context(Error::Sqlx)
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
    ) -> error_stack::Result<(), Error> {
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
        .change_context(Error::Sqlx)
        .map(|_| ())
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
