use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[cfg_attr(
    feature = "config-schema",
    derive(schemars::JsonSchema),
    schemars(deny_unknown_fields)
)]
#[serde(default)]
pub struct HgStatusConfig<'a> {
    pub format: &'a str,
    pub style: &'a str,
    pub conflicted: &'a str,
    pub shelved: &'a str,
    pub deleted: &'a str,
    pub renamed: &'a str,
    pub modified: &'a str,
    pub added: &'a str,
    pub untracked: &'a str,
    pub unknown: &'a str,
    pub disabled: bool,
}

impl Default for HgStatusConfig<'_> {
    fn default() -> Self {
        Self {
            format: "([\\[$all_status\\]]($style) )",
            style: "red bold",
            // be consistent with the git_status module by default
            conflicted: "=",
            shelved: "\\$",
            deleted: "✘",
            renamed: "»",
            modified: "!",
            added: "+",
            untracked: "?",
            unknown: "…",
            disabled: true,
        }
    }
}
