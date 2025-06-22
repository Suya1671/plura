use std::{fmt::Display, marker::PhantomData};

use slack_morphism::{errors::SlackClientError, prelude::*};
use sqlx::{Database, SqlitePool, prelude::*, types::Text};

use crate::BOT_TOKEN;

use super::trust::{Trusted, Untrusted};

#[derive(Type, Debug, PartialEq, Eq, Clone)]
pub struct Id<T> {
    pub id: Text<SlackUserId>,
    trusted: PhantomData<T>,
}

impl<'q, DB> Encode<'q, DB> for Id<Trusted>
where
    DB: Database,
    Text<SlackUserId>: Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <Text<SlackUserId> as Encode<'_, DB>>::encode_by_ref(&self.id, buf)
    }

    fn produces(&self) -> Option<<DB as sqlx::Database>::TypeInfo> {
        <Text<SlackUserId> as sqlx::Encode<'_, DB>>::produces(&self.id)
    }
}

impl<'q, DB> Decode<'q, DB> for Id<Trusted>
where
    DB: sqlx::Database,
    Text<SlackUserId>: sqlx::Decode<'q, DB>,
{
    fn decode(
        value: <DB as sqlx::Database>::ValueRef<'q>,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let id = <Text<SlackUserId> as sqlx::Decode<'_, DB>>::decode(value)?;
        Ok(Self {
            id,
            trusted: PhantomData,
        })
    }
}

impl<DB> ::sqlx::Type<DB> for Id<Trusted>
where
    DB: Database,
    Text<SlackUserId>: sqlx::Type<DB>,
{
    fn type_info() -> <DB as Database>::TypeInfo {
        <Text<SlackUserId> as sqlx::Type<DB>>::type_info()
    }
}

impl Display for Id<Trusted> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "User(ID: {}. Trusted)", self.id.0)
    }
}

impl Display for Id<Untrusted> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "User(ID: {}. Untrusted)", self.id.0)
    }
}

impl Id<Untrusted> {
    /// Transforms <@U1234|user> into an Id with the value U1234
    pub fn from_slack_escaped(escaped: &str) -> Option<Self> {
        parse_slack_user_id(escaped)
    }

    pub const fn new(id: SlackUserId) -> Self {
        Self {
            id: Text(id),
            trusted: PhantomData,
        }
    }
}

impl Id<Untrusted> {
    /// Trusts a user ID by verifying it exists
    pub async fn trust<SCHC>(
        self,
        client: &SlackClient<SCHC>,
    ) -> Result<Id<Trusted>, SlackClientError>
    where
        SCHC: SlackClientHttpConnector + Send + Sync,
    {
        let session = client.open_session(&BOT_TOKEN);

        let response = session
            .users_profile_get(&SlackApiUsersProfileGetRequest::new().with_user(self.id.0))
            .await?;

        Ok(Id {
            id: Text(response.profile.id.expect("Profile ID to exist")),
            trusted: PhantomData,
        })
    }
}

pub fn parse_slack_user_id(escaped: &str) -> Option<Id<Untrusted>> {
    escaped
        .strip_prefix("<@")
        .and_then(|s| s.strip_suffix('>'))
        .and_then(|s| s.split('|').next())
        .filter(|s| !s.is_empty())
        .filter(|s| s.starts_with('U'))
        .map(|s| SlackUserId::new(s.to_string()))
        .map(|s| Id {
            id: Text(s),
            trusted: PhantomData,
        })
}

impl From<Id<Trusted>> for SlackUserId {
    fn from(value: Id<Trusted>) -> Self {
        value.id.0
    }
}

impl From<SlackUserId> for Id<Trusted> {
    fn from(value: SlackUserId) -> Self {
        Self {
            id: Text(value),
            trusted: PhantomData,
        }
    }
}

impl<T> PartialEq<SlackUserId> for Id<T> {
    fn eq(&self, other: &SlackUserId) -> bool {
        self.id.0 == *other
    }
}

impl<T> PartialEq<Id<T>> for SlackUserId {
    fn eq(&self, other: &Id<T>) -> bool {
        *self == *other.id
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub db: SqlitePool,
}
