use std::path::{Path, PathBuf};
use std::fs;
use std::fs::{File, DirEntry};
use std::cmp::Reverse;
use crate::page::{Page, WeekPage, WeekPageV1};
use chrono::{Date, Utc, Weekday, Datelike, TimeZone};
use std::mem;
use failure;
use uuid::Uuid;

use crate::{dropbox, dropbox::{AccessToken, FileInfo}};

pub const PAGE_DIR: &str = "pages";
pub const PAGES_DIR_ON_DROPBOX: &str = "/pages";
pub const BACKUP_DIR_PREFIX: &str = "backup";

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
        let page2 = wpage2.pages.iter().find(|page2| page.id == page2.id);
        if page2.is_none() {
            // 同じIDのページが存在しない場合は追加する
            new_wpage.pages.push(page);
        }
    }

    new_wpage
}

pub fn sync(directory: &Path, client: &reqwest::Client, access_token: &AccessToken) -> Result<(), failure::Error> {
    dropbox::create_folder(client, access_token, PAGES_DIR_ON_DROPBOX)?;

    let files_on_dropbox = dropbox::list_files(client, access_token, PAGES_DIR_ON_DROPBOX)?;

    let mut files_on_local = Vec::new();
    let mut week_pages = Vec::new();
    for entry in fs::read_dir(directory.join(PAGE_DIR))? {
        let entry = entry?;

        let wpage: WeekPage = serde_json::from_reader(File::open(entry.path())?)?;
        let timestamp = wpage.uploaded_at.unwrap_or(Utc.ymd(1970, 1, 1).and_hms(0, 1, 1));
        week_pages.push(wpage);

        files_on_local.push(FileInfo {
            name: entry.path().as_path().file_name().unwrap().to_string_lossy().to_string(),
            client_modified: timestamp,
        });
    }

    let pages_dir = directory.join(PAGE_DIR);
    let gen_path = |name: &str| {
        (pages_dir.join(name), String::from(PAGES_DIR_ON_DROPBOX) + "/" + name)
    };

    for file_on_dropbox in &files_on_dropbox {
        let (local_path, dropbox_path) = gen_path(&file_on_dropbox.name);
        let file_on_local = files_on_local.iter().zip(week_pages.iter()).find(|(file, _)| file_on_dropbox.name == file.name);

        if let Some((file_on_local, wpage_on_local)) = file_on_local {
            // Dropbox上にもローカルにもファイルが存在して、タイムスタンプが異なっている場合は統合する
            if file_on_local.client_modified != file_on_dropbox.client_modified {
                let (_, page_on_dropbox) = dropbox::download_file(client, access_token, &dropbox_path)?;
                let wpage_on_dropbox: WeekPage = serde_json::from_str(&page_on_dropbox)?;

                let mut wpage = integrate(wpage_on_dropbox, wpage_on_local.clone());

                // 双方のファイルを更新する
                println!("{}を更新しています...", file_on_dropbox.name);

                let json = serde_json::to_string(&wpage)?;
                let info = dropbox::upload_file(client, access_token, &dropbox_path, json)?;

                wpage.uploaded_at = Some(info.client_modified);
                serde_json::to_writer(File::create(&local_path)?, &wpage)?;
            }
        } else {
            // Dropbox上に存在するファイルがローカルに存在しない場合はダウンロードする
            println!("{}をダウンロードしています...", file_on_dropbox.name);
            let (_, contents) = dropbox::download_file(client, access_token, &dropbox_path)?;
            fs::write(local_path, &contents)?;
        }
    }

    for (file_on_local, mut wpage) in files_on_local.iter().zip(week_pages) {
        let (local_path, dropbox_path) = gen_path(&file_on_local.name);
        let file_on_dropbox = files_on_dropbox.iter().find(|file| file_on_local.name == file.name);

        // ローカルに存在するファイルがDropbox上に存在しない場合はアップロードする
        if file_on_dropbox.is_none() {
            println!("{}をアップロードしています...", file_on_local.name);
            let json = serde_json::to_string(&wpage)?;
            let info = dropbox::upload_file(client, access_token, &dropbox_path, json)?;

            // アップロード日時を更新
            wpage.uploaded_at = Some(info.client_modified);
            serde_json::to_writer(File::create(&local_path)?, &wpage)?;
        }
    }

    Ok(())
}

// ==============================
// バックアップ
// ==============================

fn generate_backup_dir_path(base: &Path, prefix: &str, id: u32) -> PathBuf {
    base.join(&format!("{}_{}", prefix, id))
}

fn generate_backup_dir_path_not_exists(base: &Path, prefix: &str) -> (PathBuf, u32) {
    let mut id = 1;
    loop {
        let path = generate_backup_dir_path(base, prefix, id);
        if !path.exists() {
            return (path, id);
        }

        id += 1;
    }
}

pub fn create_pages_backup(directory: &Path) -> Result<u32, failure::Error> {
    let page_dir = directory.join(PAGE_DIR);
    let (backup_dir, id) = generate_backup_dir_path_not_exists(directory, BACKUP_DIR_PREFIX);

    // PAGE_DIR内のすべてのファイルをコピーする
    fs::create_dir(&backup_dir)?;
    for entry in fs::read_dir(&page_dir)? {
        let entry = entry?;
        if let Ok(ft) = entry.file_type() {
            if ft.is_file() {
                let to = backup_dir.join(entry.path().as_path().file_name().unwrap());
                fs::copy(entry.path(), &to)?;
            }
        }
    }

    Ok(id)
}

pub fn rollback(directory: &Path, id: u32) -> Result<(), failure::Error> {
    let page_dir = directory.join(PAGE_DIR);
    let backup_dir = generate_backup_dir_path(directory, BACKUP_DIR_PREFIX, id);

    fs::remove_dir_all(&page_dir)?;
    fs::create_dir(&page_dir)?;

    // backup_dir内のすべてのファイルをコピーする
    for entry in fs::read_dir(&backup_dir)? {
        let entry = entry?;
        if let Ok(ft) = entry.file_type() {
            if ft.is_file() {
                let to = page_dir.join(entry.path().as_path().file_name().unwrap());
                fs::copy(entry.path(), &to)?;
            }
        }
    }

    remove_pages_backup(directory, id)?;

    Ok(())
}

pub fn remove_pages_backup(directory: &Path, id: u32) -> Result<(), failure::Error> {
    let backup_dir = generate_backup_dir_path(directory, BACKUP_DIR_PREFIX, id);
    fs::remove_dir_all(backup_dir)?;

    Ok(())
}

// ==============================
// 修正
// ==============================

fn convert_week_page_v1_to_v2(wpage: WeekPageV1) -> WeekPage {
    WeekPage {
        pages: wpage.pages.into_iter().map(|v1| Page {
            id: Uuid::new_v4().to_string(),
            title: v1.title,
            text: v1.text,
            hidden: v1.hidden,
            created_at: v1.created_at,
            updated_at: v1.updated_at,
        }).collect(),
        uploaded_at: wpage.uploaded_at,
    }
}

pub fn fix_1_to_2(directory: &Path) -> Result<(), failure::Error> {
    // ページが格納されているディレクトリのファイルをすべて取得する
    let entries = directory.join(PAGE_DIR)
        .read_dir()?
        .filter(|entry| entry.is_ok())
        .map(|entry| entry.unwrap());

    for entry in entries {
        let file = File::open(entry.path())?;
        let wpage: WeekPageV1 = serde_json::from_reader(&file)?;
        drop(file);

        let wpage = convert_week_page_v1_to_v2(wpage);
        let file = File::create(entry.path())?;
        serde_json::to_writer(&file, &wpage)?;
    }

    Ok(())
}
