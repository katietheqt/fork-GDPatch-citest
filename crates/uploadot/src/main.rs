pub mod asset_library;
pub mod asset_store;
pub mod godot_version;
pub mod keycloak;

use crate::asset_library::AssetLibraryClient;
use crate::asset_store::AssetStoreClient;
use crate::godot_version::GodotVersion;
use clap::{Args, Parser, Subcommand};
use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use std::path::PathBuf;

pub const USER_AGENT: &str =
    "Uploadot CLI (https://github.com/GDPatch/GDPatch) <https://gdpatch.dev>";

/// CLI tool for publishing addons to Godot's Asset Store/Asset Library.
#[derive(Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Commands relating to the Asset Store.
    #[command(name = "store")]
    AssetStore(AssetStore),

    /// Commands relating to the Asset Library.
    #[command(name = "library")]
    AssetLibrary(AssetLibrary),
}

#[derive(Args)]
struct AssetStore {
    #[arg(long, env = "ASSET_STORE_USERNAME")]
    username: String,

    #[arg(long, env = "ASSET_STORE_PASSWORD")]
    password: String,

    #[clap(subcommand)]
    command: AssetStoreCommands,
}

#[derive(Subcommand)]
enum AssetStoreCommands {
    /// Uploads a package to the Godot Asset Store.
    Upload(AssetStoreUpload),
}

#[derive(Args)]
struct AssetStoreUpload {
    /// The slug for the asset (e.g. `gdpatch/editor-addon`).
    slug: String,

    /// The name of the version to upload.
    version: String,

    /// Whether to flag this release as unstable.
    #[arg(long, action, default_value_t = false)]
    unstable: bool,

    /// Path to a changelog file to upload. The Godot asset store expects Markdown formatting.
    #[arg(long)]
    changelog_path: Option<String>,

    /// The minimum Godot version supported by this asset.
    #[arg(long)]
    min_godot_version: Option<GodotVersion>,

    /// The maximum Godot version supported by this asset.
    #[arg(long)]
    max_godot_version: Option<GodotVersion>,

    /// Additional information about this version.
    #[arg(long)]
    version_notes: Option<String>,

    /// Filename to upload with.
    #[arg(short = 'f', long, default_value = "package.zip")]
    filename: String,

    /// Path to the file to upload.
    file_to_upload: PathBuf,
}

#[derive(Args)]
struct AssetLibrary {
    #[arg(long, env = "ASSET_LIBRARY_USERNAME")]
    username: String,

    #[arg(long, env = "ASSET_LIBRARY_PASSWORD")]
    password: String,

    #[clap(subcommand)]
    command: AssetLibraryCommands,
}

#[derive(Subcommand)]
enum AssetLibraryCommands {
    /// Edits an asset on the Godot Asset Library.
    Edit(AssetLibraryEdit),
}

#[derive(Args)]
struct AssetLibraryEdit {
    /// The ID of the asset.
    id: u64,

    /// A path to a file containing a new description for the asset.
    #[arg(long)]
    description: Option<String>,

    /// A new commit hash or download URL.
    #[arg(long)]
    commit: Option<String>,

    /// A new version string.
    #[arg(long)]
    version: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();

    match &args.command {
        Command::AssetStore(store) => match &store.command {
            AssetStoreCommands::Upload(upload) => upload_to_asset_store(store, upload).await?,
        },
        Command::AssetLibrary(library) => match &library.command {
            AssetLibraryCommands::Edit(edit) => edit_asset_library(library, edit).await?,
        },
    }

    Ok(())
}

async fn upload_to_asset_store(store: &AssetStore, upload: &AssetStoreUpload) -> Result<()> {
    let client = AssetStoreClient::new()?;
    client
        .authenticate_keycloak(&store.username, &store.password)
        .await?;

    let content = tokio::fs::read(&upload.file_to_upload).await?;
    let mut upload_builder = client.upload(&upload.slug, &upload.version);

    if !upload.unstable {
        upload_builder.stable();
    }

    if let Some(changelog_path) = &upload.changelog_path {
        let changelog = tokio::fs::read_to_string(changelog_path)
            .await
            .wrap_err("reading changelog")?;
        upload_builder.changelog(changelog)
    }

    if let Some(version_notes) = &upload.version_notes {
        upload_builder.version_notes(version_notes.clone());
    }

    if let Some(min_godot_version) = &upload.min_godot_version {
        upload_builder.min_godot_version(min_godot_version.clone());
    }

    if let Some(max_godot_version) = &upload.max_godot_version {
        upload_builder.min_godot_version(max_godot_version.clone());
    }

    upload_builder.finish(&upload.filename, &content).await?;

    Ok(())
}

async fn edit_asset_library(library: &AssetLibrary, edit: &AssetLibraryEdit) -> Result<()> {
    let mut client = AssetLibraryClient::new()?;
    client.login(&library.username, &library.password).await?;

    let description = if let Some(path) = &edit.description {
        let contents = tokio::fs::read_to_string(&path).await?;
        Some(contents)
    } else {
        None
    };

    client.edit(
        edit.id,
        description.as_deref(),
        edit.commit.as_deref(),
        edit.version.as_deref(),
    ).await?;

    Ok(())
}
