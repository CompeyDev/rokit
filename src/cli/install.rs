use std::collections::BTreeSet;

use anyhow::{Context, Result};
use clap::Parser;

use console::style;
use futures::{stream::FuturesUnordered, TryStreamExt};
use rokit::{
    discovery::discover_all_manifests,
    manifests::AuthManifest,
    sources::{Artifact, ArtifactProvider, ArtifactSource},
    storage::Home,
};

use crate::util::{finish_progress_bar, new_progress_bar, prompt_for_trust_specs};

/// Adds a new tool using Rokit and installs it.
#[derive(Debug, Parser)]
pub struct InstallSubcommand {
    /// Skip checking if tools have been trusted before.
    /// It is recommended to only use this on CI machines.
    #[clap(long)]
    pub no_trust_check: bool,
    /// Force install all tools, even if they are already installed.
    #[clap(long)]
    pub force: bool,
}

impl InstallSubcommand {
    pub async fn run(self, home: &Home) -> Result<()> {
        let force = self.force;

        let auth = AuthManifest::load(home.path()).await?;
        let source = ArtifactSource::new_authenticated(&auth.get_all_tokens())?;
        let manifests = discover_all_manifests(false, false).await;

        let tool_cache = home.tool_cache();
        let tool_storage = home.tool_storage();

        // 1. Gather tool specifications from all known manifests

        let tools = manifests
            .iter()
            .flat_map(|manifest| manifest.tools.clone().into_iter())
            .collect::<Vec<_>>();

        // 2. Check for trust

        // NOTE: Deduplicate tool aliases and specs since they may appear in several manifests
        let tool_aliases = tools
            .iter()
            .map(|(alias, _)| alias.clone())
            .collect::<BTreeSet<_>>();
        let tool_specs = tools
            .into_iter()
            .map(|(_, spec)| spec)
            .collect::<BTreeSet<_>>();

        let tool_specs = if self.no_trust_check {
            tool_specs
        } else {
            let (trusted_specs, untrusted_specs) = tool_specs
                .into_iter()
                .partition(|spec| tool_cache.is_trusted(&spec.clone().into_id()));
            let newly_trusted_specs = prompt_for_trust_specs(untrusted_specs).await?;
            for spec in &newly_trusted_specs {
                tool_cache.add_trust(spec.clone().into_id());
            }
            trusted_specs
                .iter()
                .chain(newly_trusted_specs.iter())
                .cloned()
                .collect::<BTreeSet<_>>()
        };

        // 3. Find artifacts, download and install them

        let pb = new_progress_bar("Installing", tool_specs.len(), 5);
        let installed_specs = tool_specs
            .into_iter()
            .map(|tool_spec| async {
                if tool_cache.is_installed(&tool_spec) && !force {
                    pb.inc(5);
                    // HACK: Force the async closure to take ownership
                    // of tool_spec by returning it from the closure
                    return anyhow::Ok(tool_spec);
                }

                let artifacts = source
                    .get_specific_release(ArtifactProvider::GitHub, &tool_spec)
                    .await?;
                pb.inc(1);

                let artifact = Artifact::sort_by_system_compatibility(&artifacts)
                    .first()
                    .cloned()
                    .with_context(|| format!("No compatible artifact found for {tool_spec}"))?;
                pb.inc(1);

                let contents = source
                    .download_artifact_contents(&artifact)
                    .await
                    .with_context(|| format!("Failed to download contents for {tool_spec}"))?;
                pb.inc(1);

                let extracted = artifact
                    .extract_contents(contents)
                    .await
                    .with_context(|| format!("Failed to extract contents for {tool_spec}"))?;
                pb.inc(1);

                tool_storage
                    .replace_tool_contents(&tool_spec, extracted)
                    .await?;
                pb.inc(1);

                tool_cache.add_installed(tool_spec.clone());
                Ok(tool_spec)
            })
            .collect::<FuturesUnordered<_>>()
            .try_collect::<Vec<_>>()
            .await?;

        // 4. Link all of the (possibly new) aliases, we do this even if the
        // tool is already installed in case the link(s) have been corrupted
        // and the user tries to re-install tools to fix it.

        pb.set_message("Linking");
        tool_aliases
            .iter()
            .map(|alias| tool_storage.create_tool_link(alias))
            .collect::<FuturesUnordered<_>>()
            .try_collect::<Vec<_>>()
            .await?;

        // 5. Finally, display a nice message to the user
        let msg = format!(
            "Installed and created link{} for {} tool{} {}",
            if installed_specs.len() == 1 { "" } else { "s" },
            style(installed_specs.len()).bold().magenta(),
            if installed_specs.len() == 1 { "" } else { "s" },
            style(format!("(took {:.2?})", pb.elapsed())).dim(),
        );
        finish_progress_bar(pb, msg);

        Ok(())
    }
}
