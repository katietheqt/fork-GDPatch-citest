use crate::godot_version::GodotVersion;
use crate::keycloak::perform_keycloak_sso;
use color_eyre::Result;
use color_eyre::eyre::{Context, OptionExt, bail, ensure};
use regex::regex;
use reqwest::{redirect, Client, ClientBuilder};
use reqwest::header::{ETAG, HeaderValue, SET_COOKIE};
use reqwest::multipart::Form;
use serde::{Deserialize, Serialize};
use std::iter::zip;
use crate::USER_AGENT;

const ASSET_STORE: &'static str = "https://store.godotengine.org";

// Multipart uploads and PUT requests sent to the CDN using presigned URLs have a maximum payload
// of 8100 KiB (7.91 MiB).
const SIZE_PER_CHUNK: usize = 8192000;

#[derive(Debug, Clone)]
pub struct AssetStoreClient(Client);

#[derive(Debug)]
pub struct AssetStoreUploadBuilder {
    client: AssetStoreClient,

    slug: String,
    version: String,

    stable: bool,

    /// A version changelog with Markdown formatting.
    changelog: String,

    /// The minimum Godot version supported by this asset.
    min_godot_version: Option<GodotVersion>,

    /// The maximum Godot version supported by this asset.
    max_godot_version: Option<GodotVersion>,

    /// Additional information about this version.
    version_notes: String,
}

fn godot_version_to_request_name(version: Option<GodotVersion>) -> &'static str {
    version.map(|v| v.to_api_name()).unwrap_or("None")
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum UploadUrlResponse {
    Error(ErrorUploadUrlResponse),
    Success(SuccessfulUploadUrlResponse),
}

#[derive(Debug, Deserialize)]
struct ErrorUploadUrlResponse {
    pub error: String,
}

#[derive(Debug, Deserialize)]
struct SuccessfulUploadUrlResponse {
    pub queue_id: u64,
    pub upload_id: String,
    pub upload_urls: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PartNumberAndETag {
    #[serde(rename = "PartNumber")]
    pub part_number: usize,

    #[serde(rename = "ETag")]
    pub etag: String,
}

impl AssetStoreUploadBuilder {
    fn new(client: &AssetStoreClient, slug: String, version: String) -> Self {
        Self {
            client: client.clone(),

            slug,
            version,

            stable: false,

            changelog: String::new(),
            min_godot_version: None,
            max_godot_version: None,
            version_notes: String::new(),
        }
    }

    pub fn changelog(&mut self, changelog: String) {
        self.changelog = changelog;
    }

    pub fn min_godot_version(&mut self, min_godot_version: GodotVersion) {
        self.min_godot_version = Some(min_godot_version);
    }

    pub fn max_godot_version(&mut self, max_godot_version: GodotVersion) {
        self.max_godot_version = Some(max_godot_version);
    }

    pub fn version_notes(&mut self, version_notes: String) {
        self.version_notes = version_notes;
    }

    pub fn stable(&mut self) {
        self.stable = true;
    }

    fn create_base_request_form(&self, csrf_token: &str) -> Form {
        let mut form = Form::new()
            .text("csrf_token", csrf_token.to_owned())
            .text("name", self.version.clone())
            .text("changelog", self.changelog.clone())
            .text(
                "min_godot_version",
                godot_version_to_request_name(self.min_godot_version),
            )
            .text(
                "max_godot_version",
                godot_version_to_request_name(self.max_godot_version),
            )
            .text("version_notes", self.version_notes.clone())
            .text("checksum", "");

        if self.stable {
            form = form.text("stable", "y")
        }

        form
    }

    async fn fetch_upload_urls(
        &self,
        csrf_token: &str,
        filename: &str,
        chunk_count: usize,
    ) -> Result<(SuccessfulUploadUrlResponse, HeaderValue)> {
        let req_body = self
            .create_base_request_form(&csrf_token)
            .text("filename", filename.to_string())
            .text("upload_chunks", chunk_count.to_string());

        let req = self
            .client
            .0
            .post(format!(
                "{ASSET_STORE}/asset/{}/version/upload_url/",
                &self.slug
            ))
            .multipart(req_body);

        let res = req.send().await?;
        let cookie = res.headers().get(SET_COOKIE).ok_or_eyre("boom")?.clone();

        let res_body = res.text().await?;
        ensure!(
            !res_body.starts_with("<!"),
            "upload_url endpoint returned HTML"
        );

        let parsed_res = serde_json::from_str::<UploadUrlResponse>(&res_body)?;
        match parsed_res {
            UploadUrlResponse::Error(err) => {
                bail!("upload_url failed: {}", err.error);
            }
            UploadUrlResponse::Success(success) => {
                let received_chunk_count = success.upload_urls.len();
                ensure!(
                    received_chunk_count == chunk_count,
                    "got the wrong amount of chunks (wanted {chunk_count}, got {received_chunk_count})"
                );

                Ok((success, cookie))
            }
        }
    }

    pub async fn finish(self, filename: &str, contents: &[u8]) -> Result<()> {
        let csrf_token = self
            .client
            .obtain_csrf_token()
            .await
            .wrap_err("obtaining CSRF token")?;

        // upload the file chunks
        let mut parts = Vec::new();
        let chunk_count = contents.len().div_ceil(SIZE_PER_CHUNK);
        let (upload_url_response, _) = self
            .fetch_upload_urls(&csrf_token, filename, chunk_count)
            .await?;

        for (chunk_idx, (upload_url, data)) in zip(
            &upload_url_response.upload_urls,
            contents.chunks(SIZE_PER_CHUNK),
        )
        .enumerate()
        {
            let response = self
                .client
                .0
                .put(upload_url)
                .body(data.to_vec())
                .send()
                .await?
                .error_for_status()?;

            let chunk_etag = response
                .headers()
                .get(ETAG)
                .ok_or_eyre("missing ETag in upload response")?
                .to_str()?
                .to_string();

            parts.push(PartNumberAndETag {
                part_number: chunk_idx + 1,
                etag: chunk_etag,
            });
        }

        // finish the version creation
        let create_request_body = self
            .create_base_request_form(&csrf_token)
            .text("filename", "")
            .text("queue_id", upload_url_response.queue_id.to_string())
            .text("upload_id", upload_url_response.upload_id.to_string())
            .text("parts", serde_json::to_string(&parts)?);

        let response = self
            .client
            .0
            .post(format!(
                "{ASSET_STORE}/asset/{}/version/create/",
                &self.slug
            ))
            .multipart(create_request_body)
            .send()
            .await?
            .error_for_status()?;

        println!("{}", response.text().await?);

        Ok(())
    }
}

impl AssetStoreClient {
    pub fn new() -> Result<Self> {
        let client = ClientBuilder::new()
            .user_agent(USER_AGENT)
            .redirect(redirect::Policy::none())
            .cookie_store(true)
            .build()?;

        Ok(Self(client))
    }

    pub async fn authenticate_keycloak(&self, username: &str, password: &str) -> Result<()> {
        perform_keycloak_sso(&self.0, &format!("{ASSET_STORE}/login"), username, password).await
    }

    pub fn upload(&self, slug: impl ToString, version: impl ToString) -> AssetStoreUploadBuilder {
        AssetStoreUploadBuilder::new(self, slug.to_string(), version.to_string())
    }

    async fn obtain_csrf_token(&self) -> Result<String> {
        let res = self
            .0
            .get(format!("{ASSET_STORE}/flag/asset/INVALID_ASSET_ID/"))
            .send()
            .await?
            .error_for_status()?;

        let page_text = res.text().await?;
        let csrf_match = regex!(r#"id="csrf_token".+?value="(?<token>.+)""#)
            .captures(&page_text)
            .and_then(|m| m.name("token"))
            .ok_or_eyre("couldn't find CSRF token")?;

        let csrf_token = csrf_match.as_str().to_string();
        Ok(csrf_token)
    }
}
