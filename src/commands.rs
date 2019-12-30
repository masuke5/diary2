use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use clap::{ArgMatches};
use failure;
use chrono::{Utc, Local, NaiveDate, TimeZone, Datelike};
use colored::*;
use reqwest::Client;

use crate::config::Config;
use crate::storage;
use crate::page::{Page, CURRENT_PAGE_VERSION};
use crate::{dropbox, dropbox::AccessToken};

#[allow(dead_code)]
pub struct Context<'a> {
    directory: PathBuf,
    config_path: PathBuf,
    config: Config,
    matches: &'a ArgMatches<'a>,
    subcommand_matches: &'a ArgMatches<'a>,
    page_version: u32,
}

impl<'a> Context<'a> {
    pub fn new(
        directory: &Path,
        config_path: &Path,
        config: Config,
        matches: &'a ArgMatches<'a>,
        subcommand_matches: &'a ArgMatches<'a>,
        page_version: u32,
    ) -> Self {
        Self {
            directory: directory.to_path_buf(),
            config_path: config_path.to_path_buf(),
            config,
            matches,
            subcommand_matches,
            page_version,
        }
    }
}

fn parse_page(text: String) -> (String, String) {
    let mut iter = text.chars();

    // 最初の行をタイトルとして取得する
    let title: String = iter.by_ref().take_while(|&ch| ch != '\n').collect();
    let title = title.trim();
    // 本文
    let text: String = iter.collect();
    let text = text.trim();

    (title.to_string(), text.to_string())
}

const TEMP_FILE_TO_EDIT: &str = "new_page.md";
const AMEND_FILE: &str = "amend_page.md";
const ACCESS_TOKEN_FILE: &str = "access_token";

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

pub fn print_page_headers<I: Iterator<Item = Page>>(iter: I) {
    for page in iter.filter(|page| !page.hidden) {
        let local = page.created_at.with_timezone(&Local);
        println!("{} {}", page.title, format!("{}", local.format("%Y/%m/%d %H:%M")).yellow());
    }
}

fn print_page(page: &Page) {
    let formatted = format!("{}", page.created_at.with_timezone(&Local).format("%Y/%m/%d %H:%M"));

    println!("## {} {}", page.title, formatted.yellow());
    println!("{}\n", page.text);
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

    print_page_headers(pages.into_iter());

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

    let (title, text) = parse_page(text);

    // 取得したタイトルが空だったらエラー
    if title.is_empty() {
        eprintln!("タイトルを取得できませんでした");
        return Ok(());
    }

    let page = Page {
        title,
        text,
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

pub fn parse_date_str(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    if s == "" {
        return None;
    }

    let now = Local::now();
    let default_year = now.year();
    let default_month = now.month();

    let value_strs: Vec<&str> = s.split(|c| c == '/' || c == '-').collect();
    let mut values = Vec::<u32>::new();
    for value_str in value_strs {
        match value_str.parse() {
            Ok(n) => values.push(n),
            Err(_) => return None,
        };
    }

    let date = match values.len() {
        1 => NaiveDate::from_ymd(default_year, default_month, values[0]),
        2 => NaiveDate::from_ymd(default_year, values[0], values[1]),
        3 => NaiveDate::from_ymd(values[0] as i32, values[1], values[2]),
        _ => return None,
    };
    
    Some(date)
}

pub fn show(ctx: Context) -> Result<(), failure::Error> {
    let date_str = ctx.subcommand_matches.value_of("date");
    let date = match date_str {
        Some(s) => {
            match parse_date_str(s) {
                Some(date) => date,
                None => {
                    eprintln!("日付を解析できませんでした");
                    return Ok(());
                },
            }
        },
        None => Utc::today().naive_local(),
    };

    // 指定された日付の週のページを取得
    let week_page = storage::get_week_page(&ctx.directory, &Utc.from_local_date(&date).unwrap())?;
    // ページが存在する時のみ出力
    if let Some(week_page) = week_page {
        // 指定された日付のページだけ抽出
        let mut pages = week_page.pages.into_iter()
            .filter(|page| !page.hidden)
            .filter(|page| page.created_at.with_timezone(&Local).date().naive_local() == date);
        
        if let Some(first_page) = pages.next() {
            println!("# {}\n", date.format("%Y/%m/%d"));

            print_page(&first_page);
            for page in pages {
                print_page(&page);
            }
        }

    }

    Ok(())
}

pub fn amend(ctx: Context) -> Result<(), failure::Error> {
    // 最新のページを取得
    let mut last_page: Page = match storage::list(&ctx.directory, 1) {
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

    // 一時ファイルへ書き込み
    let amend_file_path = ctx.directory.join(AMEND_FILE);
    let content = format!("{}\n\n{}", last_page.title, last_page.text);
    if let Err(err) = fs::write(&amend_file_path, &content) {
        eprintln!("一時ファイルへの書き込みに失敗しました: {}", err);
        return Err(From::from(err));
    }

    // エディタを開く
    if let Err(err) = execute_editor(&ctx.config.editor, &amend_file_path) {
        eprintln!("エディタの起動に失敗しました: {}", err);
        return Err(err);
    }

    // エディタで編集されたファイルを読み込む
    let text = match fs::read_to_string(&amend_file_path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("ファイルの読み込みに失敗しました: {}", err);
            return Err(From::from(err));
        },
    };


    let (title, text) = parse_page(text);

    // タイトルが空だったらエラー
    if title.is_empty() {
        eprintln!("タイトルが空です");
        return Ok(());
    }

    last_page.title = title;
    last_page.text = text;
    last_page.updated_at.push(Utc::now());

    // 書き込み
    if let Err(err) = storage::write(&ctx.directory, last_page) {
        eprintln!("ページの書き込みに失敗しました: {}", err);
        return Err(From::from(err));
    }

    Ok(())
}

pub fn search(ctx: Context) -> Result<(), failure::Error> {
    let query = ctx.subcommand_matches.value_of("query").unwrap();   
    let should_search_by_title_only = ctx.subcommand_matches.is_present("title");
    let should_search_by_text_only = ctx.subcommand_matches.is_present("text");
    let should_show_first_page = ctx.subcommand_matches.is_present("show-first");
    let limit = if should_show_first_page {
        1
    } else {
        match ctx.subcommand_matches.value_of("limit") {
            Some(limit) => match limit.parse::<u32>() {
                Ok(limit) => limit,
                Err(err) => {
                    eprintln!("--limitの値が数値ではありません");
                    return Err(From::from(err));
                },
            },
            None => ctx.config.default_list_limit,
        }
    };

    let filter = |page: &Page| -> bool {
        if should_search_by_title_only {
            page.title.contains(query)
        } else if should_search_by_text_only {
            page.text.contains(query)
        } else {
            page.title.contains(query) || page.text.contains(query)
        }
    };

    let pages = match storage::list_with_filter(&ctx.directory, limit, filter) {
        Ok(pages) => pages,
        Err(err) => {
            eprintln!("ページの取得に失敗しました: {}", err);
            return Err(From::from(err));
        },
    };

    if should_show_first_page {
        if pages.len() > 0 {
            print_page(&pages[0]);
        } else {
            eprintln!("ページが見つかりませんでした");
        }
    } else {
        print_page_headers(pages.into_iter());
    }

    Ok(())
}

pub fn auth(ctx: Context) -> Result<(), failure::Error> {
    let access_token = match dropbox::get_access_token() {
        Ok(access_token) => access_token,
        Err(err) => {
            eprintln!("アクセストークンの取得に失敗しました: {}", err);
            return Err(err.into());
        },
    };
    
    let path = ctx.directory.join(ACCESS_TOKEN_FILE);
    if let Err(err) = fs::write(path, &access_token.value) {
        eprintln!("アクセストークンの保存に失敗しました: {}", err);
        return Err(err.into());
    }

    println!("認証に成功しました");

    Ok(())
}

pub fn sync(ctx: Context) -> Result<(), failure::Error> {
    // バックアップを取っておく
    let backup_id = match storage::create_pages_backup(&ctx.directory) {
        Ok(backup_id) => backup_id,
        Err(err) => {
            eprintln!("バックアップの作成に失敗しました: {}", err);
            return Err(err.into());
        },
    };

    // アクセストークンを取得
    let path = ctx.directory.join(ACCESS_TOKEN_FILE);
    let access_token = match fs::read_to_string(path) {
        Ok(access_token) => access_token,
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                eprintln!("認証していません");
            } else {
                eprintln!("アクセストークンの取得に失敗しました: {}", err);
            }

            return Err(err.into());
        },
    };
    let access_token = AccessToken {
        value: access_token,
    };

    let client = Client::new();
    match storage::sync(&ctx.directory, &client, &access_token) {
        Ok(_) => {
            // バックアップを削除
            if let Err(err) = storage::remove_pages_backup(&ctx.directory, backup_id) {
                eprintln!("バックアップの削除に失敗しました: {}", err);
                return Err(err.into());
            }
        },
        Err(err) => {
            eprintln!("同期に失敗しました: {}", err);

            // バックアップを復元する
            if let Err(err) = storage::rollback(&ctx.directory, backup_id) {
                eprintln!("バックアップの復元に失敗しました: {}", err);
                return Err(err.into());
            }

            return Err(err.into());
        }
    };
    
    Ok(())
}

pub fn fixpage(ctx: Context) -> Result<(), failure::Error> {
    match (ctx.page_version, CURRENT_PAGE_VERSION) {
        (a, b) if a == b => {
            println!("変換は必要ありません");
        },
        _ => unreachable!(),
    };

    Ok(())
}
