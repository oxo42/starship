use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[cfg_attr(
    feature = "config-schema",
    derive(schemars::JsonSchema),
    schemars(deny_unknown_fields)
)]
#[serde(default)]
pub struct HgCommitConfig<'a> {
    pub commit_hash_length: usize,
    pub format: &'a str,
    pub style: &'a str,
    pub disabled: bool,
}

impl Default for HgCommitConfig<'_> {
    fn default() -> Self {
        Self {
            // be consistent with the git_commit module by default
            commit_hash_length: 7,
            format: "[\\($hash\\)]($style) ",
            style: "green bold",
            disabled: true,
        }
    }
}
