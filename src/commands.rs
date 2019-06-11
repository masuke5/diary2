use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use clap::{ArgMatches};
use failure;
use chrono::{Utc, Local};

use crate::config::Config;
use crate::storage;
use crate::page::Page;

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

    for page in pages.into_iter().filter(|page| !page.hidden) {
        let local = page.created_at.with_timezone(&Local);
        println!("{} ({})", page.title, local.format("%Y/%m/%d %H:%M"));
    }

    Ok(())
}

pub fn new(ctx: Context) -> Result<(), failure::Error> {
    let temp_file_path = ctx.directory.join(TEMP_FILE_TO_EDIT);

    // listコマンドで表示するかどうか
    let hidden = ctx.subcommand_matches.is_present("hidden");
    // エディタを起動する前の時刻を保存
    let created_at = Utc::now();

    // エディタを起動
    if let Err(err) = execute_editor(&ctx.config.editor, &temp_file_path) {
        eprintln!("エディタの起動に失敗しました: {}", err);
        return Err(err);
    }

    // エディタで編集されたファイルを読み込む
    let text = match fs::read_to_string(&temp_file_path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("ファイルの読み込みに失敗しました: {}", err);
            return Err(From::from(err));
        },
    };

    let mut iter = text.chars();

    // 最初の行をタイトルとして取得する
    let title: String = iter.by_ref().take_while(|&ch| ch != '\n').collect();
    let title = title.trim();
    // 本文
    let text: String = iter.collect();
    let text = text.trim();

    // 取得したタイトルが空だったらエラー
    if title.is_empty() {
        eprintln!("タイトルを取得できませんでした");
        return Ok(());
    }

    let page = Page {
        title: title.to_string(),
        text: text.to_string(),
        hidden,
        created_at,
        updated_at: vec![Utc::now()],
    };

    // 書き込み
    if let Err(err) = storage::write(&ctx.directory, page) {
        eprintln!("ページの書き込みに失敗しました: {}", err);
        return Err(From::from(err));
    }

    // ファイルを削除
    if let Err(err) = fs::remove_file(&temp_file_path) {
        eprintln!("ファイルの削除に失敗しました: {}", err);
        return Err(From::from(err));
    }

    Ok(())
}

pub fn lastdt(ctx: Context) -> Result<(), failure::Error>{ 
    // 最新のページを取得
    let last_page: Page = match storage::list(&ctx.directory, 1) {
        Ok(pages) => match pages.into_iter().next() {
            Some(page) => page,
            None => {
                eprintln!("ページがありません");
                return Ok(());
            },
        },
        Err(err) => {
            eprintln!("ページの取得に失敗しました: {}", err);
            return Err(From::from(err));
        },
    };

    println!("{}", last_page.created_at.to_rfc3339());

    Ok(())
}