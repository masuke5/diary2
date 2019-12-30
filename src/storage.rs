use std::path::{Path, PathBuf};
use std::fs;
use std::fs::{File, DirEntry};
use std::cmp::Reverse;
use crate::page::{Page, WeekPage};
use chrono::{Date, Utc, Weekday, Datelike};
use std::mem;
use failure;

use crate::{dropbox, dropbox::{AccessToken, FileInfo}};

pub const PAGE_DIR: &str = "pages";
pub const PAGES_DIR_ON_DROPBOX: &str = "/pages";

// 日曜日と土曜日の日付を取得
fn find_week(day: &Date<Utc>) -> (Date<Utc>, Date<Utc>) {
    let mut begin = day.clone();
    let mut end = day.clone();
    loop {
        match begin.weekday() {
            Weekday::Sun => break,
            _ => begin = begin.pred(),
        };
    }

    loop {
        match end.weekday() {
            Weekday::Sat => break,
            _ => end = end.succ(),
        };
    }

    (begin, end)
}

fn generate_page_filepath(directory: &Path, date: &Date<Utc>) -> PathBuf {
    let (week_begin, week_end) = find_week(date);
    let filename = format!("{}-{}.json", week_begin.format("%Y-%m-%d"), week_end.format("%Y-%m-%d"));
    let filepath = directory.join(PAGE_DIR).join(&filename);
    filepath
}

pub fn write(directory: &Path, page: Page) -> Result<(), failure::Error> {
    let filepath = generate_page_filepath(directory, &Utc::today());

    // なぜか追記される
    // let file = OpenOptions::new()
    //     .read(true)
    //     .write(true)
    //     .create(true)
    //     .open(&filepath)?;

    let exists = filepath.exists();
    let mut week_page = if exists {
        // ファイルが存在したら今週のページを読み込む
        let file = File::open(&filepath)?;
        serde_json::from_reader(file)?
    } else {
        // ファイルが存在しなかったらWeekPageを作成
        WeekPage::new()
    };

    match week_page.pages.iter().position(|old_page| page.created_at == old_page.created_at) {
         // ページが存在したら更新 
        Some(pos) => { mem::replace(&mut week_page.pages[pos], page); },
        // 存在しなかったら追加する
        None => week_page.pages.push(page),
    };

    let file = File::create(&filepath)?; 
    serde_json::to_writer(file, &week_page)?;

    Ok(())
}

pub fn list(directory: &Path, limit: u32) -> Result<Vec<Page>, failure::Error> {
    list_with_filter(directory, limit, |_| true)
}

pub fn list_with_filter<F>(directory: &Path, limit: u32, filter: F) -> Result<Vec<Page>, failure::Error>
    where F: Fn(&Page) -> bool {
    // ページが格納されているディレクトリのファイルをすべて取得する
    let mut entries: Vec<DirEntry> = directory.join(PAGE_DIR)
        .read_dir()?
        .filter(|entry| entry.is_ok())
        .map(|entry| entry.unwrap()).collect();
    // ファイル名で降順にソート
    entries.sort_by_key(|entry| Reverse(entry.file_name()));

    let mut pages: Vec<Page> = Vec::new();
    let mut count = 0u32;

    'a: for entry in entries {
        let file = File::open(entry.path())?;

        let mut week_page: WeekPage = serde_json::from_reader(file)?;
        week_page.pages.sort_by_key(|page| Reverse(page.created_at));
        for page in week_page.pages {
            if count >= limit {
                break 'a;
            }

            if filter(&page) {
                pages.push(page);
                count += 1;
            }
        }
    }

    Ok(pages)
}

pub fn get_week_page(directory: &Path, date: &Date<Utc>) -> Result<Option<WeekPage>, failure::Error> {
    let filepath = generate_page_filepath(directory, date);
    if !filepath.exists() {
        return Ok(None);
    }

    let file = File::open(filepath)?;

    let week_page: WeekPage = serde_json::from_reader(file)?;
    Ok(Some(week_page))
}

pub fn integrate(wpage1: WeekPage, wpage2: WeekPage) -> WeekPage {
    let mut new_wpage = wpage2.clone();

    for page in wpage1.pages {
        let page2 = wpage2.pages.iter().find(|page2| page.title == page2.title);
        if page2.is_none() {
            // 同じタイトルのページが存在しない場合は追加する
            // TODO: IDにする
            new_wpage.pages.push(page);
        }
    }

    new_wpage
}

pub fn sync(directory: &Path, client: &reqwest::Client, access_token: &AccessToken) -> Result<(), failure::Error> {
    dropbox::create_folder(client, access_token, PAGES_DIR_ON_DROPBOX)?;

    let files_on_dropbox = dropbox::list_files(client, access_token, PAGES_DIR_ON_DROPBOX)?;
    let files_on_local: Vec<FileInfo> = fs::read_dir(directory.join(PAGE_DIR))?
        .map(|entry| FileInfo {
            name: entry.as_ref().unwrap().path().as_path().file_name().unwrap().to_string_lossy().to_string(),
            client_modified: entry.unwrap().metadata().unwrap().modified().unwrap().into(), // UTC
        })
        .collect();

    let pages_dir = directory.join(PAGE_DIR);
    let gen_path = |name: &str| {
        (pages_dir.join(name), String::from(PAGES_DIR_ON_DROPBOX) + "/" + name)
    };

    for file_on_dropbox in &files_on_dropbox {
        let (local_path, dropbox_path) = gen_path(&file_on_dropbox.name);
        let file_on_local = files_on_local.iter().find(|file| file_on_dropbox.name == file.name);

        if let Some(_) = file_on_local {
            // Dropbox上にもローカルにもファイルが存在して、タイムスタンプが異なっている場合は統合する
            let (_, page_on_dropbox) = dropbox::download_file(client, access_token, &dropbox_path)?;
            let wpage_on_dropbox: WeekPage = serde_json::from_str(&page_on_dropbox)?;
            let wpage_on_local = serde_json::from_reader(File::open(&local_path)?)?;

            let wpage = integrate(wpage_on_dropbox, wpage_on_local);
            let json = serde_json::to_string(&wpage)?;

            // 双方のファイルを更新する
            println!("{}を更新しています...", file_on_dropbox.name);
            fs::write(&local_path, &json)?;
            dropbox::upload_file(client, access_token, &dropbox_path, json)?;
        } else {
            // Dropbox上に存在するファイルがローカルに存在しない場合はダウンロードする
            println!("{}をダウンロードしています...", file_on_dropbox.name);
            let (_, contents) = dropbox::download_file(client, access_token, &dropbox_path)?;
            fs::write(local_path, &contents)?;
        }
    }

    for file_on_local in &files_on_local {
        let (local_path, dropbox_path) = gen_path(&file_on_local.name);
        let file_on_dropbox = files_on_dropbox.iter().find(|file| file_on_local.name == file.name);

        // ローカルに存在するファイルがDropbox上に存在しない場合はアップロードする
        if file_on_dropbox.is_none() {
            println!("{}をアップロードしています...", file_on_local.name);
            let json = fs::read_to_string(local_path)?;
            dropbox::upload_file(client, access_token, &dropbox_path, json)?;
        }
    }

    Ok(())
}
