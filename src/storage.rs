use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{Date, DateTime, Datelike, Duration, Utc, Weekday};
use tokio::fs::{self, DirEntry};
use tokio::stream::StreamExt;
use uuid::Uuid;

use crate::dropbox;
use crate::dropbox::AccessToken;
use crate::page::{Page, WeekPage, WeekPageV1};

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
fn find_week(day: Date<Utc>) -> (Date<Utc>, Date<Utc>) {
    let mut begin = day;
    let mut end = day;
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

fn generate_page_filepath(directory: &Path, date: Date<Utc>) -> PathBuf {
    let (week_begin, week_end) = find_week(date);
    let filename = format!(
        "{}-{}.json",
        week_begin.format("%Y-%m-%d"),
        week_end.format("%Y-%m-%d")
    );

    directory.join(PAGE_DIR).join(&filename)
}

async fn get_edited_entries(directory: &Path) -> Result<EditedEntries> {
    let file_path = directory.join(EDITED_ENTRIES_FILE);

    if file_path.exists() {
        let json = fs::read_to_string(&file_path).await?;
        let entries: EditedEntries = serde_json::from_str(&json)?;
        Ok(entries)
    } else {
        Ok(EditedEntries::new())
    }
}

async fn update_edited_entries<F>(directory: &Path, edit: F) -> Result<()>
where
    F: FnOnce(&mut EditedEntries),
{
    let mut entries = get_edited_entries(directory).await?;

    edit(&mut entries);

    let file_path = directory.join(EDITED_ENTRIES_FILE);
    let json = serde_json::to_string(&entries)?;
    fs::write(&file_path, json).await?;

    Ok(())
}

pub async fn write(directory: &Path, page: Page) -> Result<()> {
    let filepath = generate_page_filepath(directory, Utc::today());

    // なぜか追記される
    // let file = OpenOptions::new()
    //     .read(true)
    //     .write(true)
    //     .create(true)
    //     .open(&filepath)?;

    let exists = filepath.exists();
    let mut week_page = if exists {
        // ファイルが存在したら今週のページを読み込む
        let json = fs::read_to_string(&filepath).await?;
        serde_json::from_str(&json)?
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
    })
    .await?;

    let json = serde_json::to_string(&week_page)?;
    fs::write(&filepath, &json).await?;

    Ok(())
}

pub async fn write_image(directory: &Path, image_path: &Path, file_name: &str) -> Result<()> {
    let dest = directory.join(IMAGE_DIR).join(file_name);

    update_edited_entries(directory, |entries| {
        entries
            .image_files
            .insert(dest.file_name().unwrap().to_string_lossy().to_string());
    })
    .await?;

    fs::copy(image_path, dest).await?;

    Ok(())
}

pub async fn list(directory: &Path, limit: u32) -> Result<Vec<Page>> {
    list_with_filter(directory, limit, |_| true).await
}

pub async fn list_with_filter<F>(directory: &Path, limit: u32, filter: F) -> Result<Vec<Page>>
where
    F: Fn(&Page) -> bool,
{
    // ページが格納されているディレクトリのファイルをすべて取得する
    let mut entries: Vec<DirEntry> = fs::read_dir(directory.join(PAGE_DIR))
        .await?
        .filter(|entry| entry.is_ok())
        .map(|entry| entry.unwrap())
        .collect()
        .await;

    // ファイル名で降順にソート
    entries.sort_by_key(|entry| Reverse(entry.file_name()));

    let mut pages: Vec<Page> = Vec::new();
    let mut count = 0u32;

    'a: for entry in entries {
        let json = fs::read_to_string(entry.path()).await?;

        let mut week_page: WeekPage = serde_json::from_str(&json)?;
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

pub async fn get_week_page_range(
    directory: &Path,
    start: &DateTime<Utc>,
    end: &DateTime<Utc>,
) -> Result<Vec<WeekPage>> {
    if start > end {
        return Ok(Vec::new());
    }

    let mut date = start.date();
    let end = end.date();
    let mut wpages = Vec::with_capacity((end - date).num_weeks() as usize);
    let mut last_file_path = None;

    while date <= end {
        let file_path = generate_page_filepath(directory, date);
        let json = fs::read_to_string(&file_path).await?;
        let wpage: WeekPage = serde_json::from_str(&json)?;
        wpages.push(wpage);

        date = date + Duration::days(7);
        last_file_path = Some(file_path.clone());
    }

    let last_file_path = last_file_path.unwrap();
    let file_path = generate_page_filepath(directory, end);
    if last_file_path != file_path {
        let json = fs::read_to_string(&file_path).await?;
        let wpage: WeekPage = serde_json::from_str(&json)?;
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
) -> Result<HashMap<String, FileState>>
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

pub async fn sync(
    directory: &Path,
    client: &reqwest::Client,
    access_token: &AccessToken,
) -> Result<()> {
    dropbox::create_folder(client, access_token, PAGES_DIR_ON_DROPBOX).await?;
    dropbox::create_folder(client, access_token, IMAGE_DIR_ON_DROPBOX).await?;

    let edited_entries = get_edited_entries(directory).await?;

    // ページファイルを同期

    let page_files_on_remote =
        dropbox::list_files(client, access_token, PAGES_DIR_ON_DROPBOX).await?;

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

                let (_, content) =
                    dropbox::download_file(client, access_token, &path_to_remote).await?;

                fs::write(&path_to_local, content).await?;
            }
            // アップロード
            (true, false, true) => {
                println!("{}をアップロードしています...", file_name);

                // アップロード日時を更新してからJSONに変換
                let json = fs::read_to_string(&path_to_local).await?;
                let mut wpage: WeekPage = serde_json::from_str(&json)?;
                wpage.uploaded_at = Some(Utc::now());

                let json = serde_json::to_string(&wpage)?;

                // アップロード
                dropbox::upload_file(client, access_token, &path_to_remote, json).await?;
            }
            // 統合して双方を更新
            (true, true, true) => {
                println!("{}を更新しています...", file_name);

                // ローカルのページを読み込む
                let json = fs::read_to_string(&path_to_local).await?;
                let wpage_on_local: WeekPage = serde_json::from_str(&json)?;

                // リモートのページを読み込む
                let (_, content) =
                    dropbox::download_file_to_string(client, access_token, &path_to_remote).await?;
                let wpage_on_remote = serde_json::from_str(&content)?;

                // 統合して、アップロード日時を更新
                let mut wpage = integrate(wpage_on_local, wpage_on_remote);
                wpage.uploaded_at = Some(Utc::now());

                let json = serde_json::to_string(&wpage)?;

                // ローカルのファイルを更新
                fs::write(&path_to_local, &json).await?;

                // リモートのファイルを更新
                dropbox::upload_file(client, access_token, &path_to_remote, json).await?;
            }
            (a, b, c) => unreachable!("({}, {}, {})", a, b, c),
        }
    }

    // 画像ファイルを同期

    let image_files_on_remote =
        dropbox::list_files(client, access_token, IMAGE_DIR_ON_DROPBOX).await?;

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

                let (_, content) = dropbox::download_file(client, access_token, &path_to_remote).await?;
                fs::write(&path_to_local, content).await?;
            }
            // ローカルの画像を優先する。
            // 選択できるようしてもよいかもしれない
            (true, true, true) |
            // アップロード
            (true, false, true) => {
                println!("{}をアップロードしています...", file_name);

                let image = fs::read(&path_to_local).await?;
                dropbox::upload_file(client, access_token, &path_to_remote, image).await?;
            },
            (a, b, c) => unreachable!("({}, {}, {})", a, b, c),
        }
    }

    // 更新済みリストを空にする
    update_edited_entries(directory, EditedEntries::clear).await?;

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

pub async fn create_pages_backup(directory: &Path) -> Result<u32> {
    let page_dir = directory.join(PAGE_DIR);
    let (backup_dir, id) = generate_backup_dir_path_not_exists(directory, BACKUP_DIR_PREFIX);

    // PAGE_DIR内のすべてのファイルをコピーする
    fs::create_dir(&backup_dir).await?;

    let mut entries = fs::read_dir(&page_dir).await?;
    while let Some(entry) = entries.next().await {
        let entry = entry?;
        if let Ok(ft) = entry.file_type().await {
            if ft.is_file() {
                let to = backup_dir.join(entry.path().as_path().file_name().unwrap());
                fs::copy(entry.path(), &to).await?;
            }
        }
    }

    Ok(id)
}

pub async fn rollback(directory: &Path, id: u32) -> Result<()> {
    let page_dir = directory.join(PAGE_DIR);
    let backup_dir = generate_backup_dir_path(directory, BACKUP_DIR_PREFIX, id);

    fs::remove_dir_all(&page_dir).await?;
    fs::create_dir(&page_dir).await?;

    // backup_dir内のすべてのファイルをコピーする
    let mut entries = fs::read_dir(&backup_dir).await?;
    while let Some(entry) = entries.next().await {
        let entry = entry?;
        if let Ok(ft) = entry.file_type().await {
            if ft.is_file() {
                let to = page_dir.join(entry.path().as_path().file_name().unwrap());
                fs::copy(entry.path(), &to).await?;
            }
        }
    }

    remove_pages_backup(directory, id).await?;

    Ok(())
}

pub async fn remove_pages_backup(directory: &Path, id: u32) -> Result<()> {
    let backup_dir = generate_backup_dir_path(directory, BACKUP_DIR_PREFIX, id);
    fs::remove_dir_all(backup_dir).await?;

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

pub async fn fix_1_to_2(directory: &Path) -> Result<()> {
    // ページが格納されているディレクトリのファイルをすべて取得する
    let entries = directory
        .join(PAGE_DIR)
        .read_dir()?
        .filter(|entry| entry.is_ok())
        .map(|entry| entry.unwrap());

    for entry in entries {
        let json = fs::read_to_string(entry.path()).await?;
        let wpage: WeekPageV1 = serde_json::from_str(&json)?;

        let wpage = convert_week_page_v1_to_v2(wpage);
        let json = serde_json::to_string(&wpage)?;
        fs::write(entry.path(), json).await?;
    }

    Ok(())
}
