use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use oauth2::reqwest::http_client;
use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, RedirectUrl,
    TokenResponse, TokenUrl,
};
use reqwest::{header, Client};
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::prelude::*;
use tokio::stream::StreamExt;
use url::Url;

use crate::secret;

pub struct AccessToken {
    pub value: String,
}

pub async fn get_access_token() -> Result<AccessToken> {
    let app_key = ClientId::new(secret::app_key().to_string());
    let app_secret = ClientSecret::new(secret::app_secret().to_string());
    let auth_url = AuthUrl::new("https://www.dropbox.com/oauth2/authorize".to_string()).unwrap();
    let token_url = TokenUrl::new("https://www.dropbox.com/oauth2/token".to_string()).unwrap();

    let client = BasicClient::new(app_key, Some(app_secret), auth_url, Some(token_url))
        .set_redirect_url(RedirectUrl::new("http://localhost:8888".to_string()).unwrap());

    let (authorize_url, csrf_state) = client.authorize_url(CsrfToken::new_random).url();

    println!("ブラウザーでこのリンクを開いてください: {}", authorize_url);

    let mut listener = TcpListener::bind("127.0.0.1:8888").await?;
    let mut incoming = listener.incoming();
    while let Some(stream) = incoming.next().await {
        if let Ok(mut stream) = stream {
            let (reader, mut writer) = stream.split();

            let code;
            let state;
            {
                let mut reader = BufReader::new(reader);

                let mut request_line = String::new();
                reader.read_line(&mut request_line).await?;

                let redirect_url = request_line.split_whitespace().nth(1).unwrap();
                let url =
                    Url::parse(&("http://localhost:8888".to_string() + redirect_url)).unwrap();

                let code_pair = url
                    .query_pairs()
                    .find(|pair| {
                        let &(ref key, _) = pair;
                        key == "code"
                    })
                    .unwrap();

                let (_, value) = code_pair;
                code = AuthorizationCode::new(value.into_owned());

                let state_pair = url
                    .query_pairs()
                    .find(|pair| {
                        let &(ref key, _) = pair;
                        key == "state"
                    })
                    .unwrap();

                let (_, value) = state_pair;
                state = CsrfToken::new(value.into_owned());
            }

            let message = if state.secret() != csrf_state.secret() {
                "無効なCSRFトークンです。"
            } else {
                "ターミナルに戻ってください。"
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}",
                message.len(),
                message
            );
            writer.write_all(response.as_bytes()).await?;

            let token = client.exchange_code(code).request(http_client).unwrap();
            return Ok(AccessToken {
                value: token.access_token().secret().clone(),
            });
        }
    }

    unreachable!();
}

#[derive(Debug, Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub client_modified: DateTime<Utc>,
}

pub async fn download_file_to_string(
    client: &Client,
    access_token: &AccessToken,
    path: &str,
) -> Result<(FileInfo, String)> {
    let (info, bytes) = download_file(client, access_token, path).await?;
    Ok((info, String::from_utf8_lossy(&bytes).to_string()))
}

pub async fn download_file(
    client: &Client,
    access_token: &AccessToken,
    path: &str,
) -> Result<(FileInfo, Vec<u8>)> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);
    let json = serde_json::to_string(&parameters)?;

    let res = client
        .post("https://content.dropboxapi.com/2/files/download")
        .header(
            header::AUTHORIZATION,
            &format!("Bearer {}", &access_token.value),
        )
        .header("Dropbox-API-Arg", &json)
        .send()
        .await?;

    let info: FileInfo =
        serde_json::from_str(res.headers().get("Dropbox-API-Result").unwrap().to_str()?)?;
    let bytes: Vec<u8> = res.bytes().await?.into_iter().collect();

    Ok((info, bytes))
}

pub async fn upload_file<B: Into<reqwest::Body>>(
    client: &Client,
    access_token: &AccessToken,
    path: &str,
    contents: B,
) -> Result<FileInfo> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);
    parameters.insert("mode", "overwrite");
    let json = serde_json::to_string(&parameters)?;

    let info: FileInfo = client
        .post("https://content.dropboxapi.com/2/files/upload")
        .header(
            header::AUTHORIZATION,
            &format!("Bearer {}", &access_token.value),
        )
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header("Dropbox-API-Arg", &json)
        .body(contents)
        .send()
        .await?
        .json()
        .await?;

    Ok(info)
}

#[derive(Debug, Deserialize)]
pub struct FileList {
    entries: Vec<FileInfo>,
}

pub async fn list_files(
    client: &Client,
    access_token: &AccessToken,
    path: &str,
) -> Result<Vec<FileInfo>> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);

    let list: FileList = client
        .post("https://api.dropboxapi.com/2/files/list_folder")
        .header(
            header::AUTHORIZATION,
            &format!("Bearer {}", &access_token.value),
        )
        .json(&parameters)
        .send()
        .await?
        .json()
        .await?;

    Ok(list.entries)
}

pub async fn create_folder(client: &Client, access_token: &AccessToken, path: &str) -> Result<()> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);

    let res = client
        .post("https://api.dropboxapi.com/2/files/create_folder_v2")
        .header(
            header::AUTHORIZATION,
            &format!("Bearer {}", &access_token.value),
        )
        .json(&parameters)
        .send()
        .await?;

    let j: reqwest::Result<serde_json::Value> = res.json().await;
    if j.is_err() {
        eprintln!("フォルダの作成に失敗しました");
    }

    Ok(())
}
