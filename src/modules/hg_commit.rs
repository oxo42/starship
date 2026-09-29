use super::{Context, Module, ModuleConfig, vcs};

use crate::configs::hg_commit::HgCommitConfig;
use crate::formatter::StringFormatter;

/// Creates a module with the Hg commit in the current directory
///
/// Will display the commit hash if the current directory is an hg repo
pub fn module<'a>(context: &'a Context) -> Option<Module<'a>> {
    let mut module = context.new_module("hg_commit");
    let config: HgCommitConfig = HgCommitConfig::try_load(module.config);

    // As we default to disabled=true, we have to check here after loading our config module,
    // before it was only checking against whatever is in the config starship.toml
    if config.disabled {
        return None;
    }

    vcs::discover_repo_root(context, vcs::Vcs::Hg)?;

    let hash = hg_hash(context, &config)?;

    let parsed = StringFormatter::new(config.format).and_then(|formatter| {
        formatter
            .map_style(|variable| match variable {
                "style" => Some(Ok(config.style)),
                _ => None,
            })
            .map(|variable| match variable {
                "hash" => Some(Ok(hash.as_str())),
                _ => None,
            })
            .parse(None, Some(context))
    });

    module.set_segments(match parsed {
        Ok(segments) => segments,
        Err(error) => {
            log::warn!("Error in module `hg_commit`:\n{error}");
            return None;
        }
    });

    Some(module)
}

fn hg_hash(context: &Context, config: &HgCommitConfig) -> Option<String> {
    let output = context.exec_cmd("hg", &["log", "-r", ".", "--template", "{node}"])?;
    let hash = output.stdout.trim();
    if hash.is_empty() || hash.chars().all(|c| c == '0') {
        // Empty repos report the null hash, which is not a commit to display
        return None;
    }

    Some(hash.chars().take(config.commit_hash_length).collect())
}

#[cfg(test)]
mod tests {
    use nu_ansi_term::Color;
    use std::io;
    use std::path::Path;

    use crate::test::ModuleRenderer;
    use crate::utils::{CommandOutput, create_command};

    const HG_LOG_CMD: &str = "hg log -r . --template {node}";
    const HASH_40: &str = "cbc6bacb6d39def9c6117b7b9dfd47c66c966f22";

    fn hg_mock(hash: &str) -> Option<CommandOutput> {
        Some(CommandOutput {
            stdout: hash.to_string(),
            stderr: String::new(),
        })
    }

    fn hg_repo_dir() -> io::Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".hg"))?;
        Ok(dir)
    }

    fn render_hg_commit(repo_dir: &Path, hash: &str) -> Option<String> {
        ModuleRenderer::new("hg_commit")
            .config(toml::toml! {
                [hg_commit]
                disabled = false
            })
            .path(repo_dir)
            .cmd(HG_LOG_CMD, hg_mock(hash))
            .collect()
    }

    #[test]
    fn show_nothing_on_empty_dir() -> io::Result<()> {
        let repo_dir = tempfile::tempdir()?;

        let actual = ModuleRenderer::new("hg_commit")
            .path(repo_dir.path())
            .collect();

        let expected = None;

        assert_eq!(expected, actual);
        repo_dir.close()
    }

    #[test]
    fn test_hg_commit_disabled_per_default() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = ModuleRenderer::new("hg_commit")
            .path(repo_dir.path())
            .cmd(HG_LOG_CMD, hg_mock(HASH_40))
            .collect();

        let expected = None;

        assert_eq!(expected, actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_commit_hash() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = render_hg_commit(repo_dir.path(), HASH_40);

        let expected = Some(format!("{} ", Color::Green.bold().paint("(cbc6bac)")));

        assert_eq!(expected, actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_commit_hash_len_override() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = ModuleRenderer::new("hg_commit")
            .config(toml::toml! {
                [hg_commit]
                disabled = false
                commit_hash_length = 14
            })
            .path(repo_dir.path())
            .cmd(HG_LOG_CMD, hg_mock(HASH_40))
            .collect();

        let expected = Some(format!(
            "{} ",
            Color::Green.bold().paint("(cbc6bacb6d39de)")
        ));

        assert_eq!(expected, actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_commit_hash_null_hash() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        // Empty repos report the null hash, which should render nothing
        let actual = render_hg_commit(repo_dir.path(), "0000000000000000000000000000000000000000");

        let expected = None;

        assert_eq!(expected, actual);
        repo_dir.close()
    }

    #[test]
    fn test_render_commit_hash_hg_failure() -> io::Result<()> {
        let repo_dir = hg_repo_dir()?;

        let actual = ModuleRenderer::new("hg_commit")
            .config(toml::toml! {
                [hg_commit]
                disabled = false
            })
            .path(repo_dir.path())
            .cmd(HG_LOG_CMD, None)
            .collect();

        let expected = None;

        assert_eq!(expected, actual);
        repo_dir.close()
    }

    #[test]
    #[ignore]
    fn test_render_commit_hash_real_hg() -> io::Result<()> {
        let repo_dir = tempfile::tempdir()?;
        run_hg(&["init"], repo_dir.path())?;
        std::fs::write(repo_dir.path().join("file"), "content")?;
        run_hg(&["add", "file"], repo_dir.path())?;
        run_hg(
            &[
                "commit",
                "-m",
                "initial commit",
                "-u",
                "fake user <fake@user>",
            ],
            repo_dir.path(),
        )?;

        let hg_output = create_command("hg")?
            .args(["log", "-r", ".", "--template", "{node}"])
            .current_dir(repo_dir.path())
            .output()?
            .stdout;
        let expected_hash: String = String::from_utf8_lossy(&hg_output)
            .chars()
            .take(7)
            .collect();

        let actual = ModuleRenderer::new("hg_commit")
            .config(toml::toml! {
                [hg_commit]
                disabled = false
            })
            .path(repo_dir.path())
            .collect();

        let expected = Some(format!(
            "{} ",
            Color::Green.bold().paint(format!("({expected_hash})"))
        ));

        assert_eq!(expected, actual);
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
