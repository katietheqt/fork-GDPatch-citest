use crate::USER_AGENT;
use color_eyre::Result;
use color_eyre::eyre::bail;
use reqwest::{Client, ClientBuilder, redirect};
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;
use std::fmt::{Display, Formatter};

const API_BASE: &'static str = "https://godotengine.org/asset-library/api";

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    pub error: String,
}

impl Display for ErrorResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

impl std::error::Error for ErrorResponse {}

#[derive(Debug)]
struct MaybeErrorResponse<T>(Result<T, ErrorResponse>);

impl<'de, T> Deserialize<'de> for MaybeErrorResponse<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Debug, Deserialize)]
        #[serde(untagged)]
        enum Inner<T> {
            Err(ErrorResponse),
            Ok(T),
        }

        let inner = Inner::<T>::deserialize(deserializer)?;
        Ok(Self(match inner {
            Inner::Err(err) => Err(err),
            Inner::Ok(ok) => Ok(ok),
        }))
    }
}

#[derive(Debug, Serialize)]
struct LoginRequest<'a> {
    #[serde(borrow)]
    pub username: Cow<'a, str>,

    #[serde(borrow)]
    pub password: Cow<'a, str>,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
struct LoginResponse {
    pub username: String,
    pub token: String,
}

#[derive(Debug, Serialize)]
struct EditRequest<'a> {
    #[serde(borrow)]
    pub token: Cow<'a, str>,

    #[serde(borrow)]
    pub description: Option<Cow<'a, str>>,

    #[serde(borrow)]
    pub download_commit: Option<Cow<'a, str>>,

    #[serde(borrow)]
    pub version_string: Option<Cow<'a, str>>,
}

#[derive(Debug, Deserialize)]
struct EditResponse {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct AssetLibraryClient {
    token: Option<String>,
    client: Client,
}

impl AssetLibraryClient {
    pub fn new() -> Result<Self> {
        let client = ClientBuilder::new()
            .user_agent(USER_AGENT)
            .redirect(redirect::Policy::none())
            .build()?;

        Ok(Self {
            token: None,
            client,
        })
    }

    pub async fn login(&mut self, username: &str, password: &str) -> Result<()> {
        let body = LoginRequest {
            username: username.into(),
            password: password.into(),
        };

        let response = self
            .client
            .post(&format!("{API_BASE}/login"))
            .form(&body)
            .send()
            .await?;

        let response_body = response.json::<MaybeErrorResponse<LoginResponse>>().await?;
        let response_body = response_body.0?;

        self.token = Some(response_body.token);

        Ok(())
    }

    /// Submits an edit for an addon. Returns the ID of the submitted edit on success.
    pub async fn edit(
        &mut self,
        id: u64,
        description: Option<&str>,
        download_commit: Option<&str>,
        version_string: Option<&str>,
    ) -> Result<String> {
        let Some(token) = &self.token else {
            bail!("cannot edit assets without logging in first");
        };

        let body = EditRequest {
            token: token.into(),
            description: description.map(|v| v.into()),
            download_commit: download_commit.map(|v| v.into()),
            version_string: version_string.map(|v| v.into()),
        };

        let response = self
            .client
            .post(&format!("{API_BASE}/asset/edit/{id}"))
            .json(&body)
            .send()
            .await?;

        let response_body = response.json::<MaybeErrorResponse<EditResponse>>().await?;
        let response_body = response_body.0?;
        Ok(response_body.id)
    }
}
