use std::io::{stdout, Write};

use anyhow::{bail, Context, Result};
use clap::Parser;
use console::{style, Style};
use dialoguer::{theme::ColorfulTheme, Confirm};
use mdcat::{output::Output, DEFAULT_RESOURCE_READ_LIMIT};
use pulldown_cmark_mdcat::{
    resources::FileResourceHandler, Settings, TerminalProgram, TerminalSize, Theme,
};
use syntect::parsing::SyntaxSet;

use semver::Version;

use rokit::{storage::Home, tool::ToolId};
use tempfile::NamedTempFile;

use crate::util::{find_most_compatible_artifact, CliProgressTracker};

/// Updates Rokit to the latest version.
#[derive(Debug, Parser)]
pub struct SelfUpdateSubcommand {
    /// Update even if the latest version is already installed.
    #[clap(long, hide = true)]
    pub force: bool,
}

impl SelfUpdateSubcommand {
    pub async fn run(self, home: &Home) -> Result<()> {
        let repo = env!("CARGO_PKG_REPOSITORY")
            .trim_start_matches("https://github.com/")
            .trim_end_matches(".git");
        let Ok(tool_id) = repo.parse::<ToolId>() else {
            bail!(
                "Failed to parse manifest repository URL!\
                \nThis is a bug in Rokit, please report it at:
                \n{repo}"
            );
        };

        let pt = CliProgressTracker::new_with_message("Loading", 4);
        let source = home.artifact_source().await?;

        pt.task_completed();
        pt.update_message("Fetching");

        let release_artifact = source.get_latest_release(&tool_id).await?;

        // Skip updating if we are already on the latest version
        let version_current = env!("CARGO_PKG_VERSION").parse::<Version>().unwrap();
        let version_latest = release_artifact
            .artifacts
            .first()
            .unwrap()
            .tool_spec
            .version()
            .clone();
        if version_current >= version_latest && !self.force {
            let msg = format!(
                "Rokit is already up-to-date! {}\n\n\
                The latest version is {}.",
                pt.formatted_elapsed(),
                style(&version_latest).bold().magenta(),
            );
            pt.finish_with_message(msg);
            return Ok(());
        }

        // Download the most compatible artifact - this should always exist,
        // otherwise we wouldn't be able to run Rokit in the first place...?
        pt.task_completed();
        pt.update_message("Downloading");

        let artifact = find_most_compatible_artifact(&release_artifact.artifacts, &tool_id)
            .context("No compatible Rokit artifact was found (WAT???)")?;
        let artifact_contents = source
            .download_artifact_contents(&artifact)
            .await
            .context("Failed to download latest Rokit binary")?;

        // Extract the binary contents from the artifact
        pt.task_completed();
        pt.update_message("Extracting");
        let binary_contents = artifact
            .extract_contents(artifact_contents)
            .await
            .context("Failed to extract Rokit binary from archive")?;

        // Finally, we need to replace the current binary contents and all links to it.
        pt.task_completed();
        pt.update_message("Linking");

        let storage = home.tool_storage();
        storage.replace_rokit_contents(binary_contents).await;
        storage
            .recreate_all_links()
            .await
            .context("Failed to create new tool links")?;

        // Everything went well, yay!
        let msg = format!(
            "Rokit has been updated successfully! {}\n\
            \nYou are now running version {}, updated from {}.",
            pt.formatted_elapsed(),
            style(&version_latest).bold().magenta(),
            style(&version_current).bold().magenta(),
        );
        pt.finish_with_message(msg);

        // If there is a changelog, and the user wants to see it, show it
        if let Some(changelog) = release_artifact.changelog {
            let to_show_changelog = Confirm::with_theme(&ColorfulTheme {
                active_item_prefix: style("📋 ".to_string()),
                prompt_style: Style::new(),
                ..Default::default()
            })
            .with_prompt("View changelogs for this update?")
            .interact_opt()?
            .unwrap_or_default();

            if to_show_changelog {
                let mut changelog_file = NamedTempFile::new()?;
                write!(
                    changelog_file,
                    "# Changelog - {} v{}\n{}",
                    tool_id.name(),
                    version_current,
                    changelog
                )
                .context("Failed to write changelog to temp file")?;

                println!();
                mdcat::process_file(
                    changelog_file.path().to_str().unwrap(),
                    &Settings {
                        terminal_capabilities: TerminalProgram::detect().capabilities(),
                        terminal_size: TerminalSize::detect()
                            .context("Failed to detect terminal size")?,
                        syntax_set: &SyntaxSet::load_defaults_newlines(),
                        theme: Theme::default(),
                    },
                    &FileResourceHandler::new(DEFAULT_RESOURCE_READ_LIMIT), // TODO: Maybe make this be a DispatchingResourceHandler?
                    &mut Output::Stdout(stdout()),
                )?;
            }
        }

        Ok(())
    }
}
