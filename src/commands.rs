use std::path::{Path, PathBuf};
use clap::{ArgMatches};
use failure;
use std::process::Command;

use crate::config::Config;
use crate::storage;

pub struct Context<'a> {
    directory: PathBuf,
    config_path: PathBuf,
    config: Config,
    matches: &'a ArgMatches<'a>,
    subcommand_matches: &'a ArgMatches<'a>,
}

impl<'a> Context<'a> {
    pub fn new(directory: &Path, config_path: &Path, config: Config, matches: &'a ArgMatches<'a>, subcommand_matches: &'a ArgMatches<'a>) -> Self {
        Self {
            directory: directory.to_path_buf(),
            config_path: config_path.to_path_buf(),
            config,
            matches,
            subcommand_matches,
        }
    }
}

const TEMP_FILE_TO_EDIT: &str = "new_page.md";

fn execute_editor(editor: &str, filepath: &Path) -> Result<bool, failure::Error> {
    let mut command = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(&["/c", &format!("{} {}", editor, filepath.to_string_lossy())])
            .spawn()?
    } else {
        Command::new("sh")
            .args(&["-c", &format!("{} {}", editor, filepath.to_string_lossy())])
            .spawn()?
    };

    let status = command.wait()?;
    Ok(status.success())
}

pub fn config(ctx: Context) -> Result<(), failure::Error> {
    let editor = ctx.subcommand_matches.value_of("editor").unwrap_or(&ctx.config.editor);

    // エディタを起動
    if let Err(err) = execute_editor(editor, &ctx.config_path) {
        eprintln!("エディタの起動に失敗しました: {}", err);
        return Err(err);
    }

    Ok(())
}

pub fn list(ctx: Context) -> Result<(), failure::Error> {
    let limit = match ctx.subcommand_matches.value_of("limit") {
        Some(limit) => match limit.parse::<u32>() {
            Ok(limit) => limit,
            Err(err) => {
                eprintln!("--limitの値が数値ではありません");
                return Err(From::from(err));
            },
        },
        None => ctx.config.default_list_limit,
    };

    let pages = match storage::list(&ctx.directory, limit) {
        Ok(pages) => pages,
        Err(err) => {
            eprintln!("ページの取得に失敗しました: {}", err);
            return Err(From::from(err));
        },
    };

    for page in pages {
        println!("{} ({})", page.title, page.created_at.format("%Y/%m/%d %H:%M"));
    }

    Ok(())
}