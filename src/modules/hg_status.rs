use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use super::utils::format;
use super::{Context, Module, ModuleConfig, vcs};

use crate::configs::hg_status::HgStatusConfig;
use crate::formatter::StringFormatter;
use crate::utils::CommandOutput;

const ALL_STATUS_FORMAT: &str =
    "$conflicted$shelved$unknown$deleted$renamed$modified$added$untracked";

/// Creates a module with the Hg status of the repo in the current directory
///
/// Will display symbols for the kinds of changes in the working directory, unresolved merge
/// conflicts, and shelved changes. If `hg status` fails or times out, `unknown` is displayed
/// instead of the working directory changes, so that the repo does not look clean.
pub fn module<'a>(context: &'a Context) -> Option<Module<'a>> {
    let mut module = context.new_module("hg_status");
    let config: HgStatusConfig = HgStatusConfig::try_load(module.config);

    // As we default to disabled=true, we have to check here after loading our config module,
    // before it was only checking against whatever is in the config starship.toml
    if config.disabled {
        return None;
    }

    let repo_root = vcs::discover_repo_root(context, vcs::Vcs::Hg)?;
    let info = HgStatusInfo::new(context, &repo_root);

    let parsed = StringFormatter::new(config.format).and_then(|formatter| {
        formatter
            .map_meta(|variable, _| match variable {
                "all_status" => Some(ALL_STATUS_FORMAT),
                _ => None,
            })
            .map_style(|variable| match variable {
                "style" => Some(Ok(config.style)),
                _ => None,
            })
            .map_variables_to_segments(|variable| {
                let count = |format_str, config_path, count: Option<usize>| {
                    format::count(format_str, config_path, context, count?)
                };
                let status = || info.status();

                let segments = match variable {
                    "conflicted" => {
                        count(config.conflicted, "hg_status.conflicted", info.conflicted())
                    }
                    "shelved" => count(config.shelved, "hg_status.shelved", info.shelved()),
                    "unknown" => match status() {
                        Some(_) => None,
                        None => {
                            format::text(config.unknown, "hg_status.unknown", context, |_| None)
                        }
                    },
                    "deleted" => count(
                        config.deleted,
                        "hg_status.deleted",
                        status().map(|s| s.deleted),
                    ),
                    "renamed" => count(
                        config.renamed,
                        "hg_status.renamed",
                        status().map(|s| s.renamed),
                    ),
                    "modified" => count(
                        config.modified,
                        "hg_status.modified",
                        status().map(|s| s.modified),
                    ),
                    "added" => count(config.added, "hg_status.added", status().map(|s| s.added)),
                    "untracked" => count(
                        config.untracked,
                        "hg_status.untracked",
                        status().map(|s| s.untracked),
                    ),
                    _ => None,
                };

                segments.map(Ok)
            })
            .parse(None, Some(context))
    });

    module.set_segments(match parsed {
        Ok(segments) => segments,
        Err(error) => {
            log::warn!("Error in module `hg_status`:\n{error}");
            return None;
        }
    });

    Some(module)
}

/// Lazily gathers the repository information, so that `hg` is only run for the variables that
/// are actually used in the format string.
struct HgStatusInfo<'a> {
    context: &'a Context<'a>,
    repo_root: &'a Path,
    status: OnceLock<Option<HgStatus>>,
}

impl<'a> HgStatusInfo<'a> {
    fn new(context: &'a Context, repo_root: &'a Path) -> Self {
        Self {
            context,
            repo_root,
            status: OnceLock::new(),
        }
    }

    /// Runs `hg` with `HGPLAIN` set, so that user configuration such as aliases, `[defaults]`,
    /// or `commands.status.terse` can't change the output being parsed.
    fn hg(&self, args: &[&str]) -> Option<CommandOutput> {
        self.context
            .exec_cmd_with_env("hg", args, &[("HGPLAIN", "1")])
    }

    fn status(&self) -> Option<&HgStatus> {
        self.status
            .get_or_init(|| {
                let output = self.hg(&["status", "--copies"])?;
                Some(HgStatus::parse(&output.stdout))
            })
            .as_ref()
    }

    /// Number of files with unresolved merge conflicts.
    fn conflicted(&self) -> Option<usize> {
        // Only ask hg when a merge state exists, to avoid running a command on every prompt
        let merge_dir = self.repo_root.join(".hg").join("merge");
        if !merge_dir.join("state").exists() && !merge_dir.join("state2").exists() {
            return Some(0);
        }

        let output = self.hg(&["resolve", "--list"])?;
        Some(
            output
                .stdout
                .lines()
                .filter(|line| line.starts_with("U "))
                .count(),
        )
    }

    /// Number of shelved changes.
    fn shelved(&self) -> Option<usize> {
        // Both Mercurial and Sapling keep a `<name>.patch` file for each active shelve. Once a
        // shelve has been restored it is moved to `.hg/shelve-backup`.
        let entries = match self.repo_root.join(".hg").join("shelved").read_dir() {
            Ok(entries) => entries,
            Err(_) => return Some(0),
        };

        Some(
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "patch"))
                .count(),
        )
    }
}

/// Represents the parsed output from `hg status --copies`.
#[derive(Debug, Default, PartialEq)]
struct HgStatus {
    added: usize,
    deleted: usize,
    modified: usize,
    renamed: usize,
    untracked: usize,
}

impl HgStatus {
    /// Parses the output of `hg status --copies`.
    ///
    /// Each line is a status code followed by a path. With `--copies`, the source of a copied or
    /// renamed file is printed indented on the line after the added file. A rename shows up as a
    /// copy whose source has been removed.
    fn parse(output: &str) -> Self {
        let mut status = Self::default();
        let mut added: Vec<Option<&str>> = Vec::new();
        let mut removed: HashSet<&str> = HashSet::new();
        // Whether the previous line was an added file, so a copy source can belong to it
        let mut after_added = false;

        for line in output.lines() {
            if let Some(source) = line.strip_prefix("  ") {
                if after_added && let Some(last) = added.last_mut() {
                    *last = Some(source);
                }
                after_added = false;
                continue;
            }

            let Some((code, path)) = line.split_once(' ') else {
                after_added = false;
                continue;
            };
            after_added = code == "A";

            match code {
                "M" => status.modified += 1,
                "A" => added.push(None),
                "R" => {
                    removed.insert(path);
                }
                "!" => status.deleted += 1,
                "?" => status.untracked += 1,
                _ => {}
            }
        }

        for source in added {
            match source {
                Some(source) if removed.remove(source) => status.renamed += 1,
                _ => status.added += 1,
            }
        }
        status.deleted += removed.len();

        status
    }
}

#[cfg(test)]
mod tests {
    use nu_ansi_term::Color;
    use std::io;
    use std::path::Path;

    use super::HgStatus;
    use crate::test::ModuleRenderer;
    use crate::utils::{CommandOutput, create_command};

    const HG_STATUS_CMD: &str = "hg status --copies";
    const HG_RESOLVE_CMD: &str = "hg resolve --list";

    fn hg_mock(stdout: &str) -> Option<CommandOutput> {
        Some(CommandOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
        })
    }

    fn hg_repo_dir() -> io::Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".hg"))?;
        Ok(dir)
    }

    fn renderer(repo_dir: &Path, config: toml::Table) -> ModuleRenderer<'_> {
        ModuleRenderer::new("hg_status")
            .config(config)
            .path(repo_dir)
    }

    fn enabled() -> toml::Table {
        toml::toml! {
            [hg_status]
            disabled = false
        }
    }

    fn red(text: &str) -> Option<String> {
        Some(format!("{} ", Color::Red.bold().paint(text)))
    }

    #[test]
    fn show_nothing_on_empty_dir() -> io::Result<()> {
        let repo_dir = tempfile::tempdir()?;

        let actual = renderer(repo_dir.path(), enabled()).collect();

        assert_eq!(None, actual);
        repo_dir.close()
    }

    #[test]
    fn test_hg_status_disabled_per_default() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = ModuleRenderer::new("hg_status")
            .path(repo_dir.path())
            .cmd(HG_STATUS_CMD, hg_mock("M f1\n"))
            .collect();

        assert_eq!(None, actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_clean() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = renderer(repo_dir.path(), enabled())
            .cmd(HG_STATUS_CMD, hg_mock(""))
            .collect();

        assert_eq!(None, actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_all_status() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = renderer(repo_dir.path(), enabled()).collect();

        assert_eq!(red("[✘»!+?]"), actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_each_status() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;
        let cases = [
            ("M f1\n", "[!]"),
            ("A new\n", "[+]"),
            ("R gone\n", "[✘]"),
            ("! missing\n", "[✘]"),
            ("? untracked\n", "[?]"),
            ("A new\n  old\nR old\n", "[»]"),
            // a copy keeps its source, so it is just an added file
            ("A copy\n  orig\n", "[+]"),
        ];

        for (status, expected) in cases {
            let actual = renderer(repo_dir.path(), enabled())
                .cmd(HG_STATUS_CMD, hg_mock(status))
                .collect();
            assert_eq!(red(expected), actual, "status: {status:?}");
        }

        repo_dir.close()
    }

    #[test]
    fn test_render_counts() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;
        let status = "M a\nM b\nA c\nR d\n! e\n! f\n? g\n? h\n? i\n";

        let actual = renderer(
            repo_dir.path(),
            toml::toml! {
                [hg_status]
                disabled = false
                format = "$all_status"
                deleted = "✘$count"
                modified = "!$count"
                added = "+$count"
                untracked = "?$count"
            },
        )
        .cmd(HG_STATUS_CMD, hg_mock(status))
        .collect();

        assert_eq!(Some("✘3!2+1?3".to_string()), actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_style() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = renderer(
            repo_dir.path(),
            toml::toml! {
                [hg_status]
                disabled = false
                style = "italic blue"
            },
        )
        .cmd(HG_STATUS_CMD, hg_mock("M f1\n"))
        .collect();

        let expected = Some(format!("{} ", Color::Blue.italic().paint("[!]")));
        assert_eq!(expected, actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_conflicted() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;
        let merge_dir = repo_dir.path().join(".hg").join("merge");
        std::fs::create_dir(&merge_dir)?;
        std::fs::write(merge_dir.join("state2"), "")?;

        let actual = renderer(
            repo_dir.path(),
            toml::toml! {
                [hg_status]
                disabled = false
                conflicted = "=$count"
            },
        )
        .cmd(HG_STATUS_CMD, hg_mock("M a\nM b\nM c\n"))
        .collect();

        assert_eq!(red("[=2!]"), actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_conflicted_needs_merge_state() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        // Without a merge state `hg resolve` must not be consulted
        let actual = renderer(repo_dir.path(), enabled())
            .cmd(HG_STATUS_CMD, hg_mock("M a\n"))
            .cmd(HG_RESOLVE_CMD, hg_mock("U a\n"))
            .collect();

        assert_eq!(red("[!]"), actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_shelved() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;
        let shelved_dir = repo_dir.path().join(".hg").join("shelved");
        std::fs::create_dir(&shelved_dir)?;
        for file in ["one.patch", "one.oshelve", "two.patch", "two.hg"] {
            std::fs::write(shelved_dir.join(file), "")?;
        }

        let actual = renderer(
            repo_dir.path(),
            toml::toml! {
                [hg_status]
                disabled = false
                shelved = "\\$$count"
            },
        )
        .cmd(HG_STATUS_CMD, hg_mock(""))
        .collect();

        assert_eq!(red("[$2]"), actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_hg_failure() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        // A failed or timed out `hg status` must not look like a clean repo
        let actual = renderer(repo_dir.path(), enabled())
            .cmd(HG_STATUS_CMD, None)
            .collect();

        assert_eq!(red("[…]"), actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_hg_failure_unknown_disabled() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = renderer(
            repo_dir.path(),
            toml::toml! {
                [hg_status]
                disabled = false
                unknown = ""
            },
        )
        .cmd(HG_STATUS_CMD, None)
        .collect();

        assert_eq!(None, actual);
        repo_dir.close()
    }

    #[test]
    fn test_parse_status() {
        let cases = [
            ("", HgStatus::default()),
            (
                "M f1\nA f2b\n  f2\nA new\nR f2\nR f3\n? untr\n",
                HgStatus {
                    added: 1,
                    deleted: 1,
                    modified: 1,
                    renamed: 1,
                    untracked: 1,
                },
            ),
            // paths with spaces
            (
                "A new name\n  old name\nR old name\n",
                HgStatus {
                    renamed: 1,
                    ..HgStatus::default()
                },
            ),
            // a source line after a non-added file is ignored
            (
                "A c\nM a\n  b\nR b\n",
                HgStatus {
                    added: 1,
                    modified: 1,
                    deleted: 1,
                    ..HgStatus::default()
                },
            ),
            // two copies of a removed file: one rename and one added
            (
                "A x\n  src\nA y\n  src\nR src\n",
                HgStatus {
                    added: 1,
                    renamed: 1,
                    ..HgStatus::default()
                },
            ),
            // ignored and clean files are not changes
            ("I ignored\nC clean\n", HgStatus::default()),
        ];

        for (input, expected) in cases {
            assert_eq!(expected, HgStatus::parse(input), "input: {input:?}");
        }
    }

    #[test]
    #[ignore]
    fn test_render_real_hg() -> io::Result<()> {
        let repo_dir = tempfile::tempdir()?;
        let path = repo_dir.path();
        run_hg(&["init"], path)?;
        for file in ["modified", "renamed", "removed"] {
            std::fs::write(path.join(file), "content\n")?;
        }
        run_hg(&["add", "modified", "renamed", "removed"], path)?;
        run_hg(
            &[
                "commit",
                "-m",
                "initial commit",
                "-u",
                "fake user <fake@user>",
            ],
            path,
        )?;
        std::fs::write(path.join("modified"), "changed\n")?;
        run_hg(&["mv", "renamed", "renamed2"], path)?;
        run_hg(&["rm", "removed"], path)?;
        std::fs::write(path.join("added"), "content\n")?;
        run_hg(&["add", "added"], path)?;
        std::fs::write(path.join("untracked"), "content\n")?;

        let actual = renderer(path, enabled()).collect();

        assert_eq!(red("[✘»!+?]"), actual);
        repo_dir.close()
    }

    fn run_hg(args: &[&str], repo_dir: &Path) -> io::Result<()> {
        create_command("hg")?
            .args(args)
            .current_dir(repo_dir)
            .output()?;
        Ok(())
    }
}
