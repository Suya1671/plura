use axum::{
    extract::{FromRequestParts, Query, State},
    http::{self, StatusCode, request::Parts},
};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl,
    TokenUrl, reqwest,
};
use serde::{Deserialize, Serialize};
use slack_morphism::SlackUserId;
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
        .set_redirect_uri(RedirectUrl::new(format!("{}/auth", env::base_url())).unwrap())
}

#[derive(Deserialize)]
pub struct OauthCode {
    pub code: String,
    pub state: String,
}

#[derive(Debug)]
pub struct Uri(http::Uri);

impl<S> FromRequestParts<S> for Uri
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.uri.clone()))
    }
}

#[tracing::instrument(skip_all, ret)]
pub async fn oauth_handler(
    Query(code): Query<OauthCode>,
    State(state): State<user::State>,
    Uri(_uri): Uri,
) -> String {
    let db = &state.db;

    // Retrieve the csrf token and pkce verifier
    let csrf = sqlx::query!(
        r#"
        SELECT
            owner_id as "owner_id: user::Id<Trusted>"
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
                  INSERT INTO systems (owner_id, slack_oauth_token)
                  VALUES ($1, $2)
                  ON CONFLICT (owner_id) DO UPDATE SET slack_oauth_token = $2
                "#,
                record.owner_id.id,
                user_token,
            )
            .execute(db)
            .await;

            match user {
                Ok(_user) => {
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

                    let response = format!("System for user {} authenticated!", record.owner_id.0);

                    // seemingly fails behind nest
                    // if let Err(e) = slack_client
                    //     .post_webhook_message(
                    //         &url,
                    //         &SlackApiPostWebhookMessageRequest::new(
                    //             SlackMessageContent::new()
                    //                 .with_text(response.clone()),
                    //         ),
                    //     )
                    //     .await {
                    //         error!("Error sending Slack message: {:#?}", e);
                    //     }

                    response
                }
                Err(e) => {
                    let response = format!("Error creating system: {e:#?}");

                    // seemingly fails behind nest
                    // if let Err(e) = slack_client
                    //     .post_webhook_message(
                    //         &url,
                    //         &SlackApiPostWebhookMessageRequest::new(
                    //             SlackMessageContent::new()
                    //                 .with_text(response.clone()),
                    //         ),
                    //     )
                    //     .await {
                    //         error!("Error sending Slack message: {:#?}", e);
                    //     }

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
