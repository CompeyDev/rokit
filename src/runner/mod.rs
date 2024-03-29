use std::{env::args, process::exit, str::FromStr};

use anyhow::{Context, Error, Result};

use rokit::{
    discovery::discover_tool_spec,
    storage::Home,
    system::{current_exe_name, run_interruptible},
    tool::ToolAlias,
};

mod info;
use self::info::inform_user_about_potential_fixes;

#[derive(Debug, Clone)]
pub struct Runner {
    exe_name: String,
}

impl Runner {
    pub fn new() -> Self {
        Self {
            exe_name: current_exe_name(),
        }
    }

    pub fn should_run(&self) -> bool {
        self.exe_name != env!("CARGO_BIN_NAME")
    }

    pub async fn run(&self) -> Result<()> {
        let alias = ToolAlias::from_str(&self.exe_name)?;

        let home = Home::load_from_env().await?;
        let spec = discover_tool_spec(&alias, false, false)
            .await
            .with_context(|| {
                format!(
                    "Failed to find tool '{alias}' in any project manifest file.\
                    \nAdd the tool to a project using 'rokit add' before running it."
                )
            })?;

        // TODO: Prompt for trust and install tool if not already installed

        let path = home.tool_storage().tool_path(&spec);
        let args = args().skip(1).collect::<Vec<_>>();

        let code = run_interruptible(&path, &args)
            .await
            .map_err(Error::from)
            .inspect_err(|e| inform_user_about_potential_fixes(&alias, e))?;

        exit(code);
    }
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}
