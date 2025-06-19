use std::{convert::Infallible, str::FromStr};

use error_stack::{Result, ResultExt};
use slack_morphism::prelude::*;
use sqlx::{SqlitePool, prelude::*, sqlite::SqliteQueryResult};
use tracing::{debug, warn};

use crate::id;

use super::{
    Trusted, Untrusted, system,
    trigger::{Trigger, Type},
    user,
};

id!(
    /// For an ID to be trusted, it must
    ///
    /// - Be a valid ID in the database
    /// - Be associated with a trusted system
    => Member
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
    ) -> Result<Option<Id<Trusted>>, sqlx::Error> {
        sqlx::query!(
            "SELECT
                id as 'id: Id<Trusted>'
            FROM members
            WHERE id = $1 AND system_id = $2",
            self.id,
            system_id.id
        )
        .fetch_optional(db)
        .await
        .attach_printable("Failed to validate member by system")
        .map(|res| res.map(|res| res.id))
    }

    #[tracing::instrument(skip(db))]
    pub async fn validate_by_user(
        self,
        user_id: &user::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Option<Id<Trusted>>, sqlx::Error> {
        sqlx::query!(
            "
                SELECT
                    members.id as 'id: Id<Trusted>'
                FROM members
                JOIN systems ON members.system_id = systems.id
                WHERE members.id = $1 AND systems.owner_id = $2
            ",
            self.id,
            user_id
        )
        .fetch_optional(db)
        .await
        .attach_printable("Failed to validate member by user")
        .map(|res| res.map(|res| res.id))
    }

    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_alias(
        alias: &str,
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Option<Id<Trusted>>, sqlx::Error> {
        sqlx::query!(
            "SELECT
                member_id AS 'id: Id<Trusted>'
            FROM aliases
            WHERE alias = $1 AND system_id = $2",
            alias,
            system_id
        )
        .fetch_optional(db)
        .await
        .attach_printable("Failed to fetch member id by alias")
        .map(|res| res.map(|res| res.id))
    }
}

impl Id<Trusted> {
    #[tracing::instrument(skip(db))]
    pub async fn fetch_triggers(self, db: &SqlitePool) -> Result<Vec<Trigger>, sqlx::Error> {
        Trigger::fetch_by_member_id(self, db).await
    }
}

#[derive(Debug, Clone)]
/// An untrusted member reference from an external source
pub enum MemberRef {
    Id(Id<Untrusted>),
    /// We were given a [`super::Alias`]
    Alias(String),
}

impl FromStr for MemberRef {
    type Err = Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        s.parse::<i64>().map_or_else(
            |_| Ok(Self::Alias(s.to_string())),
            |id| Ok(Self::Id(Id::new(id))),
        )
    }
}

impl MemberRef {
    #[tracing::instrument(skip(db))]
    pub async fn validate_by_system(
        &self,
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> Result<Option<Id<Trusted>>, sqlx::Error> {
        match self {
            Self::Id(id) => id
                .validate_by_system(system_id, db)
                .await
                .attach_printable("Failed to validate member reference via id and system"),
            Self::Alias(alias) => Id::fetch_by_alias(alias, system_id, db)
                .await
                .attach_printable("Failed to validate member reference via alias and system"),
        }
    }
}

// TO-DO: move SQL to rust struct
#[derive(FromRow, Debug)]
#[allow(dead_code)]
pub struct Member {
    /// The ID of the member
    pub id: Id<Trusted>,
    pub system_id: system::Id<Trusted>,
    /// The display name of the member
    pub display_name: String,
    /// The full name of the member
    pub full_name: String,
    /// Profile picture to use on messages
    pub profile_picture_url: Option<String>,
    pub title: Option<String>,
    pub pronouns: Option<String>,
    pub name_pronunciation: Option<String>,
    pub name_recording_url: Option<String>,
    pub created_at: time::PrimitiveDateTime,
}

impl Member {
    /// Fetch a member by their id
    #[tracing::instrument(skip(db))]
    pub async fn fetch_by_id(member_id: Id<Trusted>, db: &SqlitePool) -> Result<Self, sqlx::Error> {
        sqlx::query_as!(
            Member,
            r#"
            SELECT
                id as "id: Id<Trusted>",
                system_id as "system_id: system::Id<Trusted>",
                full_name,
                display_name,
                profile_picture_url,
                title,
                pronouns,
                name_pronunciation,
                name_recording_url,
                created_at as "created_at: time::PrimitiveDateTime"
            FROM members
            WHERE id = $1
            "#,
            member_id
        )
        .fetch_one(db)
        .await
        .attach_printable("Failed to fetch member by id")
    }
}

/// All information required to display a member
#[derive(FromRow, Debug)]
pub struct TriggeredMember {
    /// The ID of the member
    pub id: Id<Trusted>,
    /// The display name of the member
    pub display_name: String,
    /// Profile picture to use on messages
    pub profile_picture_url: Option<String>,
    /// The trigger text that was matched
    pub trigger_text: String,
    /// The type of trigger
    pub typ: Type,
}

impl From<Member> for TriggeredMember {
    fn from(value: Member) -> Self {
        Self {
            id: value.id,
            display_name: value.display_name,
            profile_picture_url: value.profile_picture_url,
            trigger_text: String::new(),
            typ: Type::Prefix,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct View {
    pub full_name: String,
    pub display_name: String,
    pub profile_picture_url: Option<String>,
    pub title: Option<String>,
    pub pronouns: Option<String>,
    pub name_pronunciation: Option<String>,
    pub name_recording_url: Option<String>,
}

impl View {
    /// Due to the way the slack blocks are created, all fields are moved.
    /// Clone the whole struct if you need to keep the original.
    pub fn create_blocks(self) -> Vec<SlackBlock> {
        slack_blocks![
            // display info
            some_into(
                SlackHeaderBlock::new("Display info".into()).with_block_id("display_info".into())
            ),
            some_into(SlackInputBlock::new(
                "Display name".into(),
                SlackBlockPlainTextInputElement::new("display_name".into())
                    .with_initial_value(self.display_name)
                    .into(),
            )),
            some_into(
                SlackInputBlock::new(
                    "Profile picture URL".into(),
                    SlackBlockPlainTextInputElement::new("profile_picture_url".into())
                        .with_initial_value(self.profile_picture_url.unwrap_or_default())
                        .into(),
                )
                .with_optional(true)
            ),
            // personal info
            some_into(SlackDividerBlock::new()),
            some_into(
                SlackHeaderBlock::new("Personal info".into()).with_block_id("personal_info".into())
            ),
            some_into(SlackInputBlock::new(
                "Full name".into(),
                SlackBlockPlainTextInputElement::new("full_name".into())
                    .with_initial_value(self.full_name)
                    .into(),
            )),
            some_into(
                SlackInputBlock::new(
                    "Pronouns".into(),
                    SlackBlockPlainTextInputElement::new("pronouns".into())
                        .with_initial_value(self.pronouns.unwrap_or_default())
                        .into(),
                )
                .with_optional(true)
            ),
            some_into(
                SlackInputBlock::new(
                    "Title".into(),
                    SlackBlockPlainTextInputElement::new("title".into())
                        .with_initial_value(self.title.unwrap_or_default())
                        .into(),
                )
                .with_optional(true)
            ),
            some_into(
                SlackInputBlock::new(
                    "Name pronunciation".into(),
                    SlackBlockPlainTextInputElement::new("name_pronunciation".into())
                        .with_initial_value(self.name_pronunciation.unwrap_or_default())
                        .into(),
                )
                .with_optional(true)
            ),
            some_into(
                SlackInputBlock::new(
                    "Name recording URL".into(),
                    SlackBlockPlainTextInputElement::new("name_recording_url".into())
                        .with_initial_value(self.name_recording_url.unwrap_or_default())
                        .into(),
                )
                .with_optional(true)
            )
        ]
    }

    pub fn create_add_view() -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Add a new member".into(), Self::default().create_blocks())
                .with_submit("Add".into())
                .with_external_id("create_member".into()),
        )
    }

    pub fn create_edit_view(self, member_id: Id<Trusted>) -> SlackView {
        SlackView::Modal(
            SlackModalView::new("Edit member".into(), self.create_blocks())
                .with_submit("Edit".into())
                .with_external_id(format!("edit_member_{}", member_id.id)),
        )
    }

    /// Add a member to the database
    ///
    /// Returns the id of the new member
    #[tracing::instrument(skip(db))]
    pub async fn add(
        &self,
        system_id: system::Id<Trusted>,
        db: &SqlitePool,
    ) -> error_stack::Result<i64, sqlx::Error> {
        debug!("Adding member {} to database", self.display_name);
        sqlx::query!("
            INSERT INTO members (full_name, display_name, profile_picture_url, title, pronouns, name_pronunciation, name_recording_url, system_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id
        ",
            self.full_name,
            self.display_name,
            self.profile_picture_url,
            self.title,
            self.pronouns,
            self.name_pronunciation,
            self.name_recording_url,
            system_id.id,
        )
        .fetch_one(db)
        .await
        .attach_printable("Error adding member to database")
        .map(|row| row.id)
    }

    /// Update a member in the database to match this view
    ///
    /// Returns None if the member does not exist
    #[tracing::instrument(skip(db))]
    pub async fn update(
        &self,
        member_id: Id<Trusted>,
        db: &SqlitePool,
    ) -> error_stack::Result<SqliteQueryResult, sqlx::Error> {
        sqlx::query!("
            UPDATE members
            SET full_name = $1, display_name = $2, profile_picture_url = $3, title = $4, pronouns = $5, name_pronunciation = $6, name_recording_url = $7
            WHERE id = $8
        ",
            self.full_name,
            self.display_name,
            self.profile_picture_url,
            self.title,
            self.pronouns,
            self.name_pronunciation,
            self.name_recording_url,
            member_id,
        ).execute(db).await
        .attach_printable("Error editing member in database")
    }
}

#[derive(thiserror::Error, displaydoc::Display, Debug)]
/// A field was missing from the view
pub struct MissingFieldError(String);

impl TryFrom<SlackViewState> for View {
    type Error = MissingFieldError;

    fn try_from(value: SlackViewState) -> std::result::Result<Self, Self::Error> {
        let mut view = Self::default();
        for (_id, values) in value.values {
            for (id, content) in values {
                match &*id.0 {
                    "full_name" => {
                        view.full_name = content
                            .value
                            .ok_or_else(|| MissingFieldError("display_name".to_string()))?;
                    }
                    "display_name" => {
                        view.display_name = content
                            .value
                            .ok_or_else(|| MissingFieldError("display_name".to_string()))?;
                    }
                    "profile_picture_url" => view.profile_picture_url = content.value,
                    "title" => view.title = content.value,
                    "pronouns" => view.pronouns = content.value,
                    "name_pronunciation" => view.name_pronunciation = content.value,
                    "name_recording_url" => view.name_recording_url = content.value,
                    other => {
                        warn!("Unknown field in view when parsing a member::View: {other}");
                    }
                }
            }
        }

        if view.full_name.is_empty() {
            return Err(MissingFieldError("full_name".to_string()));
        }

        if view.display_name.is_empty() {
            return Err(MissingFieldError("display_name".to_string()));
        }

        Ok(view)
    }
}

impl From<Member> for View {
    fn from(value: Member) -> Self {
        Self {
            full_name: value.full_name,
            display_name: value.display_name,
            profile_picture_url: value.profile_picture_url,
            title: value.title,
            pronouns: value.pronouns,
            name_pronunciation: value.name_pronunciation,
            name_recording_url: value.name_recording_url,
        }
    }
}
