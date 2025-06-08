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

    pub async fn validate_by_system(
        self,
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Id<Trusted>, Self> {
        let exists = sqlx::query!(
            "SELECT EXISTS(SELECT 1 FROM triggers WHERE id = $1 AND system_id = $2) AS 'exists: bool'",
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
                DELETE FROM triggers
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
pub struct Trigger {
    pub id: Id<Trusted>,
    pub member_id: member::Id<Trusted>,
    pub system_id: system::Id<Trusted>,
    pub text: String,
    pub is_prefix: bool,
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
                is_prefix
            FROM
                triggers
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
            Trigger,
            r#"
                SELECT
                    triggers.id as "id: Id<Trusted>",
                    triggers.member_id as "member_id: member::Id<Trusted>",
                    triggers.system_id as "system_id: system::Id<Trusted>",
                    triggers.text,
                    triggers.is_prefix
                FROM
                    triggers
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
            Trigger,
            r#"
            SELECT
                id as "id: Id<Trusted>",
                member_id as "member_id: member::Id<Trusted>",
                system_id as "system_id: system::Id<Trusted>",
                text,
                is_prefix
            FROM
                triggers
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
    pub text: String,
    pub is_prefix: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            text: String::new(),
            is_prefix: true,
        }
    }
}

impl View {
    pub fn create_blocks(self) -> Vec<SlackBlock> {
        let prefix_choice =
            SlackBlockChoiceItem::new(SlackBlockText::Plain("prefix".into()), "prefix".into());
        let suffix_choice =
            SlackBlockChoiceItem::new(SlackBlockText::Plain("suffix".into()), "suffix".into());

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
                        "is_prefix".into(),
                        vec![prefix_choice.clone(), suffix_choice.clone()]
                    )
                    .with_initial_option(if self.is_prefix {
                        prefix_choice
                    } else {
                        suffix_choice
                    })
                    .into(),
                )
                .with_optional(false)
            )
        )
    }

    /// Add a trigger to the database
    ///
    /// Returns the id of the new trigger
    pub async fn add(
        &self,
        system_id: system::Id<Trusted>,
        member_id: member::Id<Trusted>,
        db_pool: &SqlitePool,
    ) -> error_stack::Result<Id<Trusted>, Error> {
        debug!(
            "Adding trigger for {} (Member ID {}) to database",
            system_id, member_id
        );

        sqlx::query!(
            r#"
            INSERT INTO triggers (system_id, member_id, text, is_prefix)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
            system_id.id,
            member_id.id,
            self.text,
            self.is_prefix
        )
        .fetch_one(db_pool)
        .await
        .attach_printable("Error adding trigger to database")
        .change_context(Error::Sqlx)
        .map(|row| Id {
            id: row.id,
            trusted: std::marker::PhantomData,
        })
    }

    /// Update a trigger in the database to match this view
    pub async fn update(
        &self,
        trigger_id: Id<Trusted>,
        db: &SqlitePool,
    ) -> error_stack::Result<(), Error> {
        sqlx::query!(
            r#"
            UPDATE triggers
            SET text = $1, is_prefix = $2
            WHERE id = $3
            "#,
            self.text,
            self.is_prefix,
            trigger_id.id,
        )
        .execute(db)
        .await
        .attach_printable("Error updating trigger in database")
        .change_context(Error::Sqlx)
        .map(|_| ())
    }

    pub const fn new(trigger_text: String, is_prefix: bool) -> Self {
        Self {
            text: trigger_text,
            is_prefix,
        }
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
                    "is_prefix" => {
                        if let Some(option) = content.selected_option {
                            view.is_prefix = option.value == "prefix";
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
            is_prefix: trigger.is_prefix,
        }
    }
}
