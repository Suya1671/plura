use std::str::FromStr;

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
    /// - Be associated with a valid member or system
    => Trigger
);

impl Id<Untrusted> {
    pub const fn new(id: i64) -> Self {
        Self {
            id,
            trusted: std::marker::PhantomData,
        }
    }

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
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
pub enum Error {
    /// Error while calling the database
    Sqlx,
}

#[derive(Debug, sqlx::Type, displaydoc::Display, PartialEq, Eq)]
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
    pub async fn fetch_by_id<T>(id: Id<T>, db: &SqlitePool) -> Result<Option<Self>, sqlx::Error>
    where
        T: Trustability,
    {
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
            WHERE id = $1
            "#,
            id.id,
        )
        .fetch_optional(db)
        .await
        .attach_printable("Error fetching trigger")
    }

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
}

#[derive(Debug)]
pub struct View {
    pub text: String,
    pub typ: Type,
}

impl Default for View {
    fn default() -> Self {
        Self {
            text: String::new(),
            typ: Type::Prefix,
        }
    }
}

impl View {
    pub fn create_blocks(self) -> Vec<SlackBlock> {
        let prefix_choice = SlackBlockChoiceItem::new(
            SlackBlockText::Plain(Type::Prefix.to_string().into()),
            Type::Prefix.to_string(),
        );
        let suffix_choice = SlackBlockChoiceItem::new(
            SlackBlockText::Plain(Type::Suffix.to_string().into()),
            Type::Suffix.to_string(),
        );

        slack_blocks!(
            some_into(
                SlackHeaderBlock::new("Trigger settings".into())
                    .with_block_id("trigger_settings".into())
            ),
            some_into(
                SlackInputBlock::new(
                    "Trigger Text".into(),
                    SlackBlockPlainTextInputElement::new("trigger_text".into())
                        .with_initial_value(self.text)
                        .into(),
                )
                .with_optional(false)
            ),
            some_into(
                SlackInputBlock::new(
                    "Trigger Type".into(),
                    SlackBlockRadioButtonsElement::new(
                        "type".into(),
                        vec![prefix_choice, suffix_choice]
                    )
                    .with_initial_option(SlackBlockChoiceItem::new(
                        SlackBlockText::Plain(Type::Prefix.to_string().into()),
                        Type::Prefix.to_string(),
                    ))
                    .into(),
                )
                .with_optional(false)
            )
        )
    }

    /// Add a trigger to the database
    ///
    /// Returns the id of the new trigger
    #[tracing::instrument(skip(db))]
    pub async fn add(
        &self,
        system_id: system::Id<Trusted>,
        member_id: member::Id<Trusted>,
        db: &SqlitePool,
    ) -> error_stack::Result<Id<Trusted>, Error> {
        debug!(
            "Adding trigger for {} (Member ID {}) to database",
            system_id, member_id
        );

        sqlx::query!(
            r#"
            INSERT INTO triggers (system_id, member_id, text, typ)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
            system_id.id,
            member_id.id,
            self.text,
            self.typ
        )
        .fetch_one(db)
        .await
        .attach_printable("Error adding trigger to database")
        .change_context(Error::Sqlx)
        .map(|row| Id {
            id: row.id,
            trusted: std::marker::PhantomData,
        })
    }

    /// Update a trigger in the database to match this view
    #[tracing::instrument(skip(db))]
    pub async fn update(
        &self,
        trigger_id: Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE triggers
            SET text = $1, typ = $2
            WHERE id = $3
            "#,
            self.text,
            self.typ,
            trigger_id.id,
        )
        .execute(db)
        .await
        .attach_printable("Error updating trigger in database")
    }

    pub fn create_add_view(self, member_id: member::Id<Trusted>) -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Add a new trigger".into(), self.create_blocks())
                .with_submit("Add".into())
                .with_external_id(format!("create_trigger_{}", member_id.id)),
        )
    }

    pub fn create_edit_view(self, trigger_id: Id<Trusted>) -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Edit trigger".into(), self.create_blocks())
                .with_submit("Save".into())
                .with_external_id(format!("edit_trigger_{}", trigger_id.id)),
        )
    }
}

impl From<SlackViewState> for View {
    fn from(value: SlackViewState) -> Self {
        let mut view = Self::default();
        for (_id, values) in value.values {
            for (id, content) in values {
                match &*id.0 {
                    "trigger_text" => {
                        if let Some(text) = content.value {
                            view.text = text;
                        }
                    }
                    "typ" => {
                        if let Some(option) = content.selected_option {
                            match option.value.parse::<Type>() {
                                Ok(typ) => view.typ = typ,
                                Err(error) => warn!(?error, "Error parsing trigger type"),
                            }
                        }
                    }
                    other => {
                        warn!("Unknown field in view when parsing a trigger::View: {other}");
                    }
                }
            }
        }

        view
    }
}

impl From<Trigger> for View {
    fn from(trigger: Trigger) -> Self {
        Self {
            text: trigger.text,
            typ: trigger.typ,
        }
    }
}
