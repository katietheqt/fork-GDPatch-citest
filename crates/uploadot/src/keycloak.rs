use color_eyre::eyre::{Context, OptionExt, Result, bail};
use regex::regex;
use reqwest::header::LOCATION;
use reqwest::{Client, Response, Url};
use serde::Serialize;
use std::borrow::Cow;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeycloakAuthenticateParams<'a> {
    username: &'a str,
    password: &'a str,
    credential_id: Cow<'static, str>,
}

fn get_redirect_url(response: &Response) -> Result<Url> {
    response
        .headers()
        .get(LOCATION)
        .ok_or_eyre("missing Location header")?
        .to_str()
        .wrap_err("non-ASCII Location header")?
        .parse::<Url>()
        .wrap_err("invalid Location header")
}

/// Performs Keycloak SSO using a username and password. The provided `initial_url` should redirect
/// to a Keycloak portal and the client needs a cookie jar.
pub async fn perform_keycloak_sso(client: &Client, initial_url: &str, username: &str, password: &str) -> Result<()> {
    let auth_redirect_response = client.get(initial_url).send().await?.error_for_status()?;

    if !auth_redirect_response.status().is_redirection() {
        bail!("expected Keycloak redirect");
    }

    let login_url = get_redirect_url(&auth_redirect_response)?;

    let login_page_response = client.get(login_url).send().await?.error_for_status()?;

    // find the login URL from the form
    let login_page = login_page_response.text().await?;
    let authenticate_url = regex!(r#"kc-form-login".+action="(?<target>.+?)""#)
        .captures(&login_page)
        .and_then(|captures| captures.name("target"))
        .ok_or_eyre("couldn't find login URL")?
        .as_str()
        .parse::<Url>()
        .wrap_err("invalid login URL")?;

    // send an authenticate request
    let authenticate_params = KeycloakAuthenticateParams {
        username,
        password,
        credential_id: "".into(),
    };

    let authenticate_response = client.post(authenticate_url)
        .form(&authenticate_params)
        .send()
        .await?;

    if !authenticate_response.status().is_redirection() {
        bail!("expected authentication to redirect");
    }

    let client_authenticate_url = get_redirect_url(&authenticate_response)?;
    client.get(client_authenticate_url).send().await?.error_for_status()?;

    Ok(())
}