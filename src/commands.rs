use std::borrow::Cow;
use std::fs;
use std::iter;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context as _, Result};
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Utc};
use clap::ArgMatches;
use colored::*;
use comrak::{markdown_to_html, ComrakOptions};
use reqwest::Client;
use uuid::Uuid;

use crate::config::Config;
use crate::page::{convert_image_paths_in_text, Page, CURRENT_PAGE_VERSION};
use crate::storage;
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

fn parse_page(
    text: String,
    image_prefix: &str,
) -> Result<(String, String, Vec<(PathBuf, String)>)> {
    if text.trim().is_empty() {
        return Err(anyhow!("キャンセルされました"));
    }

    let mut iter = text.chars();

    // 最初の行をタイトルとして取得する
    let title: String = iter.by_ref().take_while(|&ch| ch != '\n').collect();
    let title = title.trim();
    if title.is_empty() {
        return Err(anyhow!("タイトルが空です"));
    }

    // 本文
    let text: String = iter.collect();
    let text = text.trim();

    let (text, images) = convert_image_paths_in_text(text, |s| {
        if s.starts_with(image_prefix) {
            s.to_string()
        } else {
            format!("{}{}", image_prefix, s)
        }
    });

    Ok((title.to_string(), text.to_string(), images))
}

fn generate_image_prefix(created_at: &DateTime<Utc>) -> String {
    created_at.format("%Y-%m-%d_%H-%M-%S-%f_").to_string()
}

const TEMP_FILE_TO_EDIT: &str = "new_page.md";
const AMEND_FILE: &str = "amend_page.md";
const ACCESS_TOKEN_FILE: &str = "access_token";
const FILE_FOR_SHOWING: &str = "show.html";

const DEFAULT_COMMAND_OPEN: &str = {
    #[cfg(target_os = "windows")]
    {
        "start"
    }
    #[cfg(target_os = "macos")]
    {
        "open"
    }
    #[cfg(target_os = "linux")]
    {
        "xdg-open"
    }
};

fn execute_command(s: &str) -> Command {
    if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(&["/c", s]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(&["-c", s]);
        command
    }
}

fn execute_editor(editor: &str, filepath: &Path) -> Result<bool> {
    let mut command =
        execute_command(&format!("{} {}", editor, filepath.to_string_lossy())).spawn()?;

    let status = command.wait()?;
    Ok(status.success())
}

fn open_file_with_associated(file: &Path, command: Option<&str>) -> Result<()> {
    let command = command.unwrap_or(DEFAULT_COMMAND_OPEN);

    Command::new(command)
        .arg(format!("{}", file.display()))
        .status()?;

    Ok(())
}

pub fn config(ctx: Context) -> Result<()> {
    let editor = ctx
        .subcommand_matches
        .value_of("editor")
        .unwrap_or(&ctx.config.editor);

    // エディタを起動
    if let Err(err) = execute_editor(editor, &ctx.config_path) {
        eprintln!("エディタの起動に失敗しました: {}", err);
        return Err(err);
    }

    Ok(())
}

pub fn print_page_headers<I: Iterator<Item = Page>>(iter: I) {
    for page in iter {
        let local = page.created_at.with_timezone(&Local);
        println!(
            "{} {}",
            page.title,
            format!("{}", local.format("%Y/%m/%d %H:%M")).yellow()
        );
    }
}

fn print_page(page: &Page) {
    let formatted = format!(
        "{}",
        page.created_at
            .with_timezone(&Local)
            .format("%Y/%m/%d %H:%M")
    );

    println!("## {} {}", page.title, formatted.yellow());
    println!("{}\n", page.text);
}

pub fn list(ctx: Context) -> Result<()> {
    let limit = match ctx.subcommand_matches.value_of("limit") {
        Some(limit) => limit
            .parse::<u32>()
            .context("--limitの値が数値ではありません")?,
        None => ctx.config.default_list_limit,
    };

    let pages = storage::list_with_filter(&ctx.directory, limit, |page| !page.hidden)
        .context("ページの取得に失敗しました")?;

    print_page_headers(pages.into_iter());

    Ok(())
}

pub fn new(ctx: Context) -> Result<()> {
    let temp_file_path = ctx.directory.join(TEMP_FILE_TO_EDIT);

    // listコマンドで表示するかどうか
    let hidden = ctx.subcommand_matches.is_present("hidden");
    // エディタを起動する前の時刻を保存
    let created_at = Utc::now();

    // エディタを起動
    execute_editor(&ctx.config.editor, &temp_file_path)
        .context("エディタの起動に失敗しました: {}")?;

    // エディタで編集されたファイルを読み込む
    let text = fs::read_to_string(&temp_file_path).with_context(|| {
        format!(
            "ファイル `{}` の読み込みに失敗しました",
            temp_file_path.display()
        )
    })?;

    let image_prefix = generate_image_prefix(&created_at);

    let (title, text, images) =
        parse_page(text, &image_prefix).context("ページのパースに失敗しました")?;

    let page = Page {
        id: Uuid::new_v4().to_string(),
        title,
        text,
        hidden,
        created_at,
        updated_at: vec![Utc::now()],
    };

    // 画像をコピー
    for (original_path, file_name) in &images {
        if !original_path.exists() {
            return Err(anyhow!(
                "`{}` が存在しません",
                original_path.to_string_lossy()
            ));
        } else {
            storage::write_image(&ctx.directory, original_path, file_name).with_context(|| {
                format!("`{}` の書き込みに失敗しました", original_path.display())
            })?;
        }
    }

    // ページを書き込む
    storage::write(&ctx.directory, page).context("ページの書き込みに失敗しました")?;

    // 一時ファイルを削除
    // 書き込みに失敗したときはここまで到達できないので削除されない
    fs::remove_file(&temp_file_path).with_context(|| {
        format!(
            "ファイル `{}` の削除に失敗しました",
            temp_file_path.display()
        )
    })?;

    Ok(())
}

pub fn lastdt(ctx: Context) -> Result<()> {
    // 最新のページを取得
    let pages = storage::list(&ctx.directory, 1).context("ページの取得に失敗しました")?;

    let last_page = match pages.into_iter().next() {
        Some(page) => page,
        None => return Err(anyhow!("ページがありません")),
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

fn escape(raw: &str) -> Cow<str> {
    let mut s = String::new();

    let mut next_range = 0..0;
    for ch in raw.chars() {
        next_range = next_range.start..next_range.end + ch.len_utf8();
        let escaped = match ch {
            '>' => "&gt;",
            '<' => "&lt;",
            '&' => "&amp;",
            '\'' => "&#39;",
            '"' => "&quot;",
            _ => continue,
        };

        s.push_str(&raw[next_range.start..next_range.end - ch.len_utf8()]);
        s.push_str(escaped);

        next_range = next_range.end..next_range.end;
    }

    if s.is_empty() {
        raw.into()
    } else {
        s.push_str(&raw[next_range.start..next_range.end]);
        s.into()
    }
}

fn pages_to_html<I>(directory: &Path, date: &NaiveDate, pages: I) -> String
where
    I: Iterator<Item = Page>,
{
    let mut html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
<title>{}</title>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/github-markdown-css/4.0.0/github-markdown.min.css">
<style>
.markdown-body {{
  box-sizing: border-box;
  min-width: 200px;
  max-width: 980px;
  margin: 0 auto;
  padding: 45px;
  font-family: "Noto Sans CJK JP", "Yu Gothic", sans-serif;
}}

.markdown-body pre, .markdown-body code {{
  font-family: monospace;
}}

@media (max-width: 767px) {{
  .markdown-body {{
    padding: 15px;
  }}
}}
</style>
</head>
<body>
"#,
        escape(&format!("{}", date.format("%Y/%m/%d")))
    );

    let options = ComrakOptions {
        hardbreaks: false,
        smart: true,
        github_pre_lang: true,
        width: 100,
        default_info_string: None,
        unsafe_: false,
        ext_tagfilter: false,
        ext_table: true,
        ext_strikethrough: true,
        ext_autolink: false,
        ext_tasklist: true,
        ext_superscript: false,
        ext_header_ids: None,
        ext_footnotes: false,
        ext_description_lists: false,
    };

    for page in pages {
        // 画像URLを修正
        let (text, _) = convert_image_paths_in_text(&page.text, |s| {
            let path = directory.join(storage::IMAGE_DIR).join(s);
            format!("{}", path.display())
        });

        let body_html = markdown_to_html(&text, &options);
        html.push_str(&format!(
            r#"<article class="page markdown-body">
<h1>{}</h1>
{}
</article>
"#,
            escape(&page.title),
            &body_html
        ));
    }

    html.push_str(
        r#"</body>
</html>
"#,
    );

    html
}

fn show_with_browser(directory: &Path, command: Option<&str>, s: &str) -> Result<()> {
    let file_path = directory.join(FILE_FOR_SHOWING);
    fs::write(&file_path, s)
        .with_context(|| format!("`{}` へのHTMLの書き込みに失敗しました", file_path.display()))?;

    open_file_with_associated(&file_path, command)
        .with_context(|| format!("`{}` をブラウザで開けませんでした", file_path.display()))?;

    Ok(())
}

fn show_page_with_browser<I>(
    directory: &Path,
    command: Option<&str>,
    date: &NaiveDate,
    pages: I,
) -> Result<()>
where
    I: Iterator<Item = Page>,
{
    let html = pages_to_html(directory, date, pages);
    show_with_browser(directory, command, &html)?;

    Ok(())
}

pub fn show(ctx: Context) -> Result<()> {
    let date_str = ctx.subcommand_matches.value_of("date");
    let date = match date_str {
        Some(s) => parse_date_str(s).context("日付を解析できませんでした")?,
        None => Local::today().naive_local(),
    };

    // 日付をUTCに変換
    let datetime = date.and_hms(0, 0, 0);
    let datetime = Local
        .from_local_datetime(&datetime)
        .unwrap()
        .with_timezone(&Utc);

    // 指定された日付の週のページを取得
    let week_pages = storage::get_week_page_range(
        &ctx.directory,
        &datetime,
        &(datetime + chrono::Duration::days(1)),
    )
    .context("ページの取得に失敗しました")?;

    let mut pages: Box<dyn Iterator<Item = Page>> = Box::new(iter::empty());

    for week_page in week_pages {
        // 指定された日付のページだけ抽出
        pages = Box::new(
            pages.chain(
                week_page
                    .pages
                    .into_iter()
                    .filter(|page| !page.hidden)
                    .filter(|page| {
                        page.created_at.with_timezone(&Local).date().naive_local() == date
                    }),
            ),
        );
    }

    if ctx.subcommand_matches.is_present("stdout") {
        if let Some(first_page) = pages.next() {
            println!("# {}\n", date.format("%Y/%m/%d"));

            print_page(&first_page);
            for page in pages {
                print_page(&page);
            }
        }
    } else {
        show_page_with_browser(
            &ctx.directory,
            ctx.config.browser.as_ref().map(|s| s.as_ref()),
            &date,
            pages,
        )?;
    }

    Ok(())
}

pub fn amend(ctx: Context) -> Result<()> {
    // 最新のページを取得
    let pages = storage::list(&ctx.directory, 1).context("ページの取得に失敗しました")?;

    let mut last_page = match pages.into_iter().next() {
        Some(page) => page,
        None => return Err(anyhow!("ページがありません")),
    };

    // 一時ファイルへ書き込む
    let amend_file_path = ctx.directory.join(AMEND_FILE);
    let content = format!("{}\n\n{}", last_page.title, last_page.text);
    fs::write(&amend_file_path, &content).with_context(|| {
        format!(
            "一時ファイル `{}` への書き込みに失敗しました",
            amend_file_path.display()
        )
    })?;

    // エディタを開く
    execute_editor(&ctx.config.editor, &amend_file_path)
        .context("エディタの起動に失敗しました: {}")?;

    // エディタで編集されたファイルを読み込む
    let text = fs::read_to_string(&amend_file_path).with_context(|| {
        format!(
            "一時ファイル `{}` の読み込みに失敗しました",
            amend_file_path.display()
        )
    })?;

    let image_prefix = generate_image_prefix(&last_page.created_at);

    let (title, text, images) =
        parse_page(text, &image_prefix).context("ページのパースに失敗しました")?;

    // タイトルが空だったらエラー
    if title.is_empty() {
        return Err(anyhow!("タイトルが空です"));
    }

    last_page.title = title;
    last_page.text = text;
    last_page.updated_at.push(Utc::now());

    // 画像を書き込む
    for (original_path, file_name) in &images {
        if original_path.exists() {
            storage::write_image(&ctx.directory, original_path, file_name).with_context(|| {
                format!("`{}` の書き込みに失敗しました", original_path.display())
            })?;
        } else {
            eprintln!(
                "`{}` が存在しなかったため無視しました",
                original_path.to_string_lossy()
            );
        }
    }

    // ページを書き込む
    storage::write(&ctx.directory, last_page).context("ページの書き込みに失敗しました")?;

    Ok(())
}

pub fn search(ctx: Context) -> Result<()> {
    let query = ctx.subcommand_matches.value_of("query").unwrap();
    let should_search_by_title_only = ctx.subcommand_matches.is_present("title");
    let should_search_by_text_only = ctx.subcommand_matches.is_present("text");
    let should_show_first_page = ctx.subcommand_matches.is_present("show-first");
    let show_stdout = ctx.subcommand_matches.is_present("stdout");

    // --show-firstが指定されている場合は1つだけ検索すればよい
    let limit = if should_show_first_page {
        1
    } else {
        // limitが指定されていない場合は設定のdefault_list_limitを使う
        match ctx.subcommand_matches.value_of("limit") {
            Some(limit) => limit
                .parse::<u32>()
                .context("--limitの値が数値ではありません")?,
            None => ctx.config.default_list_limit,
        }
    };

    // オプションを元にクロージャを生成
    let filter = |page: &Page| -> bool {
        if page.hidden {
            return false;
        }

        if should_search_by_title_only {
            page.title.contains(query)
        } else if should_search_by_text_only {
            page.text.contains(query)
        } else {
            page.title.contains(query) || page.text.contains(query)
        }
    };

    // 検索
    let pages = storage::list_with_filter(&ctx.directory, limit, filter)
        .context("ページの取得に失敗しました")?;

    if should_show_first_page {
        if !pages.is_empty() {
            if show_stdout {
                print_page(&pages[0]);
            } else {
                // 表示
                show_page_with_browser(
                    &ctx.directory,
                    ctx.config.browser.as_ref().map(|s| s.as_ref()),
                    &pages[0]
                    .created_at
                    .with_timezone(&Local)
                    .date()
                    .naive_local(),
                    iter::once(pages[0].clone()),
                )?;
            }
        } else {
            eprintln!("ページが見つかりませんでした");
        }
    } else {
        print_page_headers(pages.into_iter());
    }

    Ok(())
}

pub fn auth(ctx: Context) -> Result<()> {
    let access_token =
        dropbox::get_access_token().context("アクセストークンの取得に失敗しました")?;

    let path = ctx.directory.join(ACCESS_TOKEN_FILE);
    fs::write(path, &access_token.value).context("アクセストークンの保存に失敗しました")?;

    println!("認証に成功しました");

    Ok(())
}

pub fn sync(ctx: Context) -> Result<()> {
    // バックアップを取っておく
    let backup_id =
        storage::create_pages_backup(&ctx.directory).context("バックアップの作成に失敗しました")?;

    // アクセストークンを取得
    let path = ctx.directory.join(ACCESS_TOKEN_FILE);
    if !path.exists() {
        println!("認証していません。");
        println!("`diary2 auth` を実行して認証してください。");
    }

    let access_token =
        fs::read_to_string(path).context("アクセストークンの取得に失敗しました: {}")?;

    let access_token = AccessToken {
        value: access_token,
    };

    let client = Client::new();
    match storage::sync(&ctx.directory, &client, &access_token) {
        Ok(_) => {
            // バックアップを削除
             storage::remove_pages_backup(&ctx.directory, backup_id)
                .context("バックアップの削除に失敗しました")?;
        }
        Err(err) => {
            // バックアップを復元する
            storage::rollback(&ctx.directory, backup_id)
                .context("バックアップの復元に失敗しました")?;

            return Err(err).context("同期に失敗しました");
        }
    };

    Ok(())
}

pub fn fixpage(ctx: Context) -> Result<()> {
    match (ctx.page_version, CURRENT_PAGE_VERSION) {
        (a, b) if a == b => {
            println!("変換は必要ありません");
        }
        (1, 2) => {
            if let Err(err) = storage::fix_1_to_2(&ctx.directory) {
                eprintln!("修正に失敗しました: {}", err);
                return Err(err.into());
            }
        }
        _ => unreachable!(),
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape() {
        assert_eq!("&lt;html&gt;", escape("<html>").to_string());
        assert_eq!(
            "これは&lt;html&gt;タグ",
            escape("これは<html>タグ").to_string()
        );
        assert_eq!("html", escape("html").to_string());
    }
}
