use crate::page::{Page, WeekPage, WeekPageV1};
use chrono::{Date, DateTime, Datelike, Duration, Utc, Weekday};
use failure;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::{DirEntry, File};
use std::mem;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::dropbox;
use crate::dropbox::AccessToken;

#[derive(Debug, Serialize, Deserialize)]
struct EditedEntries {
    page_files: HashSet<String>,
    image_files: HashSet<String>,
}

impl EditedEntries {
    fn new() -> Self {
        EditedEntries {
            page_files: HashSet::new(),
            image_files: HashSet::new(),
        }
    }

    fn clear(&mut self) {
        self.page_files.clear();
        self.image_files.clear();
    }
}

#[derive(Debug)]
struct FileState {
    exists_on_local: bool,
    exists_on_remote: bool,
    is_edited: bool,
}

pub const PAGE_DIR: &str = "pages";
pub const PAGES_DIR_ON_DROPBOX: &str = "/pages";
pub const IMAGE_DIR: &str = "images";
pub const IMAGE_DIR_ON_DROPBOX: &str = "/images";
pub const BACKUP_DIR_PREFIX: &str = "backup";
pub const EDITED_ENTRIES_FILE: &str = "edited_entries.json";

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
    let filename = format!(
        "{}-{}.json",
        week_begin.format("%Y-%m-%d"),
        week_end.format("%Y-%m-%d")
    );
    let filepath = directory.join(PAGE_DIR).join(&filename);
    filepath
}

fn get_edited_entries(directory: &Path) -> Result<EditedEntries, failure::Error> {
    let file_path = directory.join(EDITED_ENTRIES_FILE);

    if file_path.exists() {
        let file = File::open(&file_path)?;
        let entries: EditedEntries = serde_json::from_reader(file)?;
        Ok(entries)
    } else {
        Ok(EditedEntries::new())
    }
}

fn update_edited_entries<F>(directory: &Path, edit: F) -> Result<(), failure::Error>
where
    F: FnOnce(&mut EditedEntries),
{
    let mut entries = get_edited_entries(directory)?;

    edit(&mut entries);

    let file_path = directory.join(EDITED_ENTRIES_FILE);
    let file = File::create(&file_path)?;
    serde_json::to_writer(file, &entries)?;

    Ok(())
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

    // syncコマンドでアップロードされるようにuploaded_atを消す
    week_page.uploaded_at = None;

    match week_page
        .pages
        .iter()
        .position(|old_page| page.created_at == old_page.created_at)
    {
        // ページが存在したら更新
        Some(pos) => {
            mem::replace(&mut week_page.pages[pos], page);
        }
        // 存在しなかったら追加する
        None => week_page.pages.push(page),
    };

    update_edited_entries(directory, |entries| {
        entries
            .page_files
            .insert(filepath.file_name().unwrap().to_string_lossy().to_string());
    })?;

    let file = File::create(&filepath)?;
    serde_json::to_writer(file, &week_page)?;

    Ok(())
}

pub fn write_image(
    directory: &Path,
    image_path: &Path,
    file_name: &str,
) -> Result<(), failure::Error> {
    let dest = directory.join(IMAGE_DIR).join(file_name);

    update_edited_entries(directory, |entries| {
        entries
            .image_files
            .insert(dest.file_name().unwrap().to_string_lossy().to_string());
    })?;

    fs::copy(image_path, dest)?;

    Ok(())
}

pub fn list(directory: &Path, limit: u32) -> Result<Vec<Page>, failure::Error> {
    list_with_filter(directory, limit, |_| true)
}

pub fn list_with_filter<F>(
    directory: &Path,
    limit: u32,
    filter: F,
) -> Result<Vec<Page>, failure::Error>
where
    F: Fn(&Page) -> bool,
{
    // ページが格納されているディレクトリのファイルをすべて取得する
    let mut entries: Vec<DirEntry> = directory
        .join(PAGE_DIR)
        .read_dir()?
        .filter(|entry| entry.is_ok())
        .map(|entry| entry.unwrap())
        .collect();
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

pub fn get_week_page_range(
    directory: &Path,
    start: &DateTime<Utc>,
    end: &DateTime<Utc>,
) -> Result<Vec<WeekPage>, failure::Error> {
    if start > end {
        return Ok(Vec::new());
    }

    let mut date = start.date();
    let end = end.date();
    let mut wpages = Vec::with_capacity((end - date).num_weeks() as usize);
    let mut last_file_path = None;

    while date <= end {
        let file_path = generate_page_filepath(directory, &date);
        let file = File::open(&file_path)?;
        let wpage: WeekPage = serde_json::from_reader(&file)?;
        wpages.push(wpage);

        date = date + Duration::days(7);
        last_file_path = Some(file_path.clone());
    }

    let last_file_path = last_file_path.unwrap();
    let file_path = generate_page_filepath(directory, &end);
    if last_file_path != file_path {
        let file = File::open(&file_path)?;
        let wpage: WeekPage = serde_json::from_reader(&file)?;
        wpages.push(wpage);
    }

    Ok(wpages)
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

fn get_file_map<'a, IR>(
    local_files_dir: &Path,
    edited_files: &HashSet<String>,
    files_on_remote: IR,
) -> Result<HashMap<String, FileState>, failure::Error>
where
    IR: Iterator<Item = &'a str>,
{
    let mut result = HashMap::default();

    for file_name in files_on_remote {
        let exists_on_local = local_files_dir.join(file_name).exists();
        if exists_on_local {
            if edited_files.contains(file_name) {
                result.insert(
                    file_name.to_string(),
                    FileState {
                        exists_on_local: true,
                        exists_on_remote: true,
                        is_edited: true,
                    },
                );
            }
        } else {
            assert!(!edited_files.contains(file_name));

            result.insert(
                file_name.to_string(),
                FileState {
                    exists_on_local: false,
                    exists_on_remote: true,
                    is_edited: false,
                },
            );
        }
    }

    for file_name in edited_files {
        assert!(local_files_dir.join(file_name).exists());

        result.entry(file_name.to_string()).or_insert(FileState {
            exists_on_local: true,
            exists_on_remote: false,
            is_edited: true,
        });
    }

    Ok(result)
}

pub fn sync(
    directory: &Path,
    client: &reqwest::Client,
    access_token: &AccessToken,
) -> Result<(), failure::Error> {
    dropbox::create_folder(client, access_token, PAGES_DIR_ON_DROPBOX)?;
    dropbox::create_folder(client, access_token, IMAGE_DIR_ON_DROPBOX)?;

    let edited_entries = get_edited_entries(directory)?;

    // ページファイルを同期

    let page_files_on_remote = dropbox::list_files(client, access_token, PAGES_DIR_ON_DROPBOX)?;

    let page_dir = directory.join(PAGE_DIR);
    let file_map = get_file_map(
        &page_dir,
        &edited_entries.page_files,
        page_files_on_remote.iter().map(|f| f.name.as_ref()),
    )?;

    for (file_name, state) in file_map {
        let path_to_remote = format!("{}/{}", PAGES_DIR_ON_DROPBOX, file_name);
        let path_to_local = page_dir.join(&file_name);

        match (
            state.exists_on_local,
            state.exists_on_remote,
            state.is_edited,
        ) {
            // ダウンロード
            (false, true, false) => {
                println!("{}をダウンロードしています...", file_name);

                let (_, content) = dropbox::download_file(client, access_token, &path_to_remote)?;

                fs::write(&path_to_local, content)?;
            }
            // アップロード
            (true, false, true) => {
                println!("{}をアップロードしています...", file_name);

                // アップロード日時を更新してからJSONに変換
                let file = File::open(&path_to_local)?;
                let mut wpage: WeekPage = serde_json::from_reader(file)?;
                wpage.uploaded_at = Some(Utc::now());

                let json = serde_json::to_string(&wpage)?;

                // アップロード
                dropbox::upload_file(client, access_token, &path_to_remote, json)?;
            }
            // 統合して双方を更新
            (true, true, true) => {
                println!("{}を更新しています...", file_name);

                // ローカルのページを読み込む
                let file = File::open(&path_to_local)?;
                let wpage_on_local: WeekPage = serde_json::from_reader(file)?;

                // リモートのページを読み込む
                let (_, content) =
                    dropbox::download_file_to_string(client, access_token, &path_to_remote)?;
                let wpage_on_remote = serde_json::from_str(&content)?;

                // 統合して、アップロード日時を更新
                let mut wpage = integrate(wpage_on_local, wpage_on_remote);
                wpage.uploaded_at = Some(Utc::now());

                let json = serde_json::to_string(&wpage)?;

                // ローカルのファイルを更新
                fs::write(&path_to_local, &json)?;

                // リモートのファイルを更新
                dropbox::upload_file(client, access_token, &path_to_remote, json)?;
            }
            (a, b, c) => unreachable!("({}, {}, {})", a, b, c),
        }
    }

    // 画像ファイルを同期

    let image_files_on_remote = dropbox::list_files(client, access_token, IMAGE_DIR_ON_DROPBOX)?;

    let image_dir = directory.join(IMAGE_DIR);
    let file_map = get_file_map(
        &image_dir,
        &edited_entries.image_files,
        image_files_on_remote.iter().map(|f| f.name.as_ref()),
    )?;

    for (file_name, state) in file_map {
        let path_to_remote = format!("{}/{}", IMAGE_DIR_ON_DROPBOX, file_name);
        let path_to_local = image_dir.join(&file_name);

        match (
            state.exists_on_local,
            state.exists_on_remote,
            state.is_edited,
        ) {
            // ダウンロード
            (false, true, false) => {
                println!("{}をダウンロードしています...", file_name);

                let (_, content) = dropbox::download_file(client, access_token, &path_to_remote)?;
                fs::write(&path_to_local, content)?;
            }
            // ローカルの画像を優先する。
            // 選択できるようしてもよいかもしれない
            (true, true, true) |
            // アップロード
            (true, false, true) => {
                println!("{}をアップロードしています...", file_name);

                let image = fs::read(&path_to_local)?;
                dropbox::upload_file(client, access_token, &path_to_remote, image)?;
            },
            (a, b, c) => unreachable!("({}, {}, {})", a, b, c),
        }
    }

    // 更新済みリストを空にする
    update_edited_entries(directory, EditedEntries::clear)?;

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
        pages: wpage
            .pages
            .into_iter()
            .map(|v1| Page {
                id: Uuid::new_v4().to_string(),
                title: v1.title,
                text: v1.text,
                hidden: v1.hidden,
                created_at: v1.created_at,
                updated_at: v1.updated_at,
            })
            .collect(),
        uploaded_at: wpage.uploaded_at,
    }
}

pub fn fix_1_to_2(directory: &Path) -> Result<(), failure::Error> {
    // ページが格納されているディレクトリのファイルをすべて取得する
    let entries = directory
        .join(PAGE_DIR)
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
