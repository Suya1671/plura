use axum::{
    extract::{FromRequestParts, Query, State},
    http::{StatusCode, request::Parts},
};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl,
    TokenUrl, reqwest,
};
use serde::{Deserialize, Serialize};
use slack_morphism::{SlackClient, SlackUserId, prelude::*};
use tracing::error;

use crate::{
    env,
    models::{trust::Trusted, user},
};

#[derive(Serialize, Deserialize, Debug)]
pub struct SlackAuthedUser {
    pub id: String,
    pub scope: String,
    pub access_token: String,
    pub token_type: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SlackTokenFields {
    pub authed_user: SlackAuthedUser,
}
impl oauth2::ExtraTokenFields for SlackTokenFields {}

pub type SlackOauthClient<
    HasAuthUrl = EndpointSet,
    HasDeviceAuthUrl = EndpointNotSet,
    HasIntrospectionUrl = EndpointNotSet,
    HasRevocationUrl = EndpointNotSet,
    HasTokenUrl = EndpointSet,
> = oauth2::Client<
    oauth2::StandardErrorResponse<oauth2::basic::BasicErrorResponseType>,
    oauth2::StandardTokenResponse<SlackTokenFields, oauth2::basic::BasicTokenType>,
    oauth2::basic::BasicTokenIntrospectionResponse,
    oauth2::StandardRevocableToken,
    oauth2::basic::BasicRevocationErrorResponse,
    HasAuthUrl,
    HasDeviceAuthUrl,
    HasIntrospectionUrl,
    HasRevocationUrl,
    HasTokenUrl,
>;

pub fn create_oauth_client() -> SlackOauthClient {
    SlackOauthClient::new(ClientId::new(env::slack_client_id()))
        .set_client_secret(ClientSecret::new(env::slack_client_secret()))
        .set_auth_uri(AuthUrl::new("https://slack.com/oauth/v2/authorize".to_owned()).unwrap())
        .set_token_uri(TokenUrl::new("https://slack.com/api/oauth.v2.access".to_owned()).unwrap())
        .set_redirect_uri(
            RedirectUrl::new("https://slack-system-bot.wobbl.in/auth".to_owned()).unwrap(),
        )
}

#[derive(Deserialize)]
pub struct OauthCode {
    pub code: String,
    pub state: String,
}

pub struct Url(url::Url);

impl<S> FromRequestParts<S> for Url
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let url = url::Url::parse(&parts.uri.to_string())
            .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid URL"))?;
        Ok(Self(url))
    }
}

#[tracing::instrument(skip_all, ret)]
pub async fn oauth_handler(
    Query(code): Query<OauthCode>,
    State(state): State<user::State>,
    Url(url): Url,
) -> String {
    let db = &state.db;

    // Retrieve the csrf token and pkce verifier
    let csrf = sqlx::query!(
        r#"
        SELECT
            owner_id as "owner_id: user::Id<Trusted>",
            name
        FROM
            system_oauth_process
        WHERE csrf = $1
        "#,
        code.state
    )
    .fetch_optional(db)
    .await;

    match csrf {
        Ok(Some(record)) => {
            let client = create_oauth_client();
            let slack_client = SlackClient::new(SlackClientHyperConnector::new().unwrap());

            let response = client
                .exchange_code(AuthorizationCode::new(code.code))
                .request_async(&reqwest::Client::new())
                .await
                .unwrap();

            let user_token = response.extra_fields().authed_user.access_token.clone();
            let user_id = response.extra_fields().authed_user.id.clone();
            let user_id: SlackUserId = user_id.into();

            if user_id != record.owner_id {
                return "CSRF token doesn't match the user".to_owned();
            }

            let user = sqlx::query!(
                r#"
                  INSERT INTO systems (name, owner_id, slack_oauth_token)
                  VALUES ($1, $2, $3)
                  RETURNING name
                "#,
                record.name,
                record.owner_id.id,
                user_token,
            )
            .fetch_one(db)
            .await;

            match user {
                Ok(user) => {
                    sqlx::query!(
                        r#"
                        DELETE FROM system_oauth_process
                        WHERE csrf = $1
                        "#,
                        code.state
                    )
                    .execute(db)
                    .await
                    .unwrap();

                    let response = format!("System {} created!", user.name);

                    if let Err(e) = slack_client
                        .post_webhook_message(
                            &url,
                            &SlackApiPostWebhookMessageRequest::new(
                                SlackMessageContent::new()
                                    .with_text(response.clone()),
                            ),
                        )
                        .await {
                            error!("Error sending Slack message: {:#?}", e);
                        }

                    response
                }
                Err(e) => {
                    let response = format!("Error creating system: {e:#?}");

                    if let Err(e) = slack_client
                        .post_webhook_message(
                            &url,
                            &SlackApiPostWebhookMessageRequest::new(
                                SlackMessageContent::new()
                                    .with_text(response.clone()),
                            ),
                        )
                        .await {
                            error!("Error sending Slack message: {:#?}", e);
                        }

                    error!("{response}");
                    response
                }
            }
        }
        Ok(None) => {
            "CSRF couldn't be linked to a user. Theres a middleman attack at play or the dev (Suya1671) didn't save the token properly".to_owned()
        }
        Err(e) => {
            error!("Error fetching CSRF token: {:#?}", e);
            "Error fetching CSRF token".to_owned()
        }
    }
}
