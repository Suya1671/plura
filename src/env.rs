use menv::require_envs;

require_envs! {
    (assert_env_vars, any_set, gen_help);

    slack_app_token, "SLACK_APP_TOKEN", String,
    "SLACK_APP_TOKEN should be set to the bot's app token";

    slack_bot_token, "SLACK_BOT_TOKEN", String,
    "SLACK_BOT_TOKEN should be set to the bot's user token";

    slack_client_id, "SLACK_CLIENT_ID", String,
    "SLACK_CLIENT_ID should be set to the client ID for oauth";

    slack_client_secret, "SLACK_CLIENT_SECRET", String,
    "SLACK_CLIENT_SECRET should be set to the client secret for oauth";

    slack_signing_secret, "SLACK_SIGNING_SECRET", String,
    "SLACK_SIGNING_SECRET should be set to the signing secret for verifying slack requests";

    database_url, "DATABASE_URL", String,
    "DATABASE_URL should be set to a postgres database connection string";

    encryption_key?, "ENCRYPTION_KEY", String,
    "ENCRYPTION_KEY can be optionally set to a key for encrypting and decrypting the database";
}
