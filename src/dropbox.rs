use std::collections::HashMap;
use std::io::{Write, BufRead, BufReader};
use std::net::TcpListener;

use oauth2::reqwest::http_client;
use oauth2::{AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, RedirectUrl, TokenResponse, TokenUrl, basic::BasicClient};
use url::Url;
use chrono::{Utc, DateTime};
use failure;
use reqwest::{header, Client};

use crate::secret;

pub struct AccessToken {
    pub value: String,
}

pub fn get_access_token() -> Result<AccessToken, failure::Error> {
    let app_key = ClientId::new(secret::app_key().to_string());
    let app_secret = ClientSecret::new(secret::app_secret().to_string());
    let auth_url = AuthUrl::new("https://www.dropbox.com/oauth2/authorize".to_string()).unwrap();
    let token_url = TokenUrl::new("https://www.dropbox.com/oauth2/token".to_string()).unwrap();

    let client = BasicClient::new(
        app_key,
        Some(app_secret),
        auth_url,
        Some(token_url),
    ).set_redirect_url(RedirectUrl::new("http://localhost:8888".to_string()).unwrap());

    let (authorize_url, csrf_state) = client.authorize_url(CsrfToken::new_random).url();

    println!("ブラウザーでこのリンクを開いてください: {}", authorize_url);

    let listener = TcpListener::bind("127.0.0.1:8888")?;
    for stream in listener.incoming() {
        if let Ok(mut stream) = stream {
            let code;
            let state;
            {
                let mut reader = BufReader::new(&stream);

                let mut request_line = String::new();
                reader.read_line(&mut request_line)?;

                let redirect_url = request_line.split_whitespace().nth(1).unwrap();
                let url = Url::parse(&("http://localhost:8888".to_string() + redirect_url)).unwrap();

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
            stream.write_all(response.as_bytes())?;

            let token = client.exchange_code(code).request(http_client)?;
            return Ok(AccessToken { value: token.access_token().secret().clone() });
        }
    }

    unreachable!();
}

#[derive(Debug, Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub client_modified: DateTime<Utc>,
}

pub fn download_file(client: &Client, access_token: &AccessToken, path: &str) -> Result<(FileInfo, String), failure::Error> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);
    let json = serde_json::to_string(&parameters)?;

    let mut res = client.post("https://content.dropboxapi.com/2/files/download")
        .header(header::AUTHORIZATION, &format!("Bearer {}", &access_token.value))
        .header("Dropbox-API-Arg", &json)
        .send()?;

    let info: FileInfo = serde_json::from_str(res.headers().get("Dropbox-API-Result").unwrap().to_str()?)?;
    let contents = res.text()?;

    Ok((info, contents))
}

pub fn upload_file(client: &Client, access_token: &AccessToken, path: &str, contents: String) -> Result<FileInfo, failure::Error> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);
    parameters.insert("mode", "overwrite");
    let json = serde_json::to_string(&parameters)?;

    let info: FileInfo = client.post("https://content.dropboxapi.com/2/files/upload")
        .header(header::AUTHORIZATION, &format!("Bearer {}", &access_token.value))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header("Dropbox-API-Arg", &json)
        .body(contents)
        .send()?
        .json()?;

    Ok(info)
}

#[derive(Debug, Deserialize)]
pub struct FileList {
    entries: Vec<FileInfo>,
}

pub fn list_files(client: &Client, access_token: &AccessToken, path: &str) -> Result<Vec<FileInfo>, failure::Error> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);

    let list: FileList = client.post("https://api.dropboxapi.com/2/files/list_folder")
        .header(header::AUTHORIZATION, &format!("Bearer {}", &access_token.value))
        .json(&parameters)
        .send()?
        .json()?;

    Ok(list.entries)
}

pub fn create_folder(client: &Client, access_token: &AccessToken, path: &str) -> Result<(), failure::Error> {
    let mut parameters = HashMap::new();
    parameters.insert("path", path);

    let mut res = client.post("https://api.dropboxapi.com/2/files/create_folder_v2")
        .header(header::AUTHORIZATION, &format!("Bearer {}", &access_token.value))
        .json(&parameters)
        .send()?;

    let j: reqwest::Result<serde_json::Value> = res.json();
    if let Err(_) = j {
        eprintln!("フォルダの作成に失敗しました: {}", res.text()?);
    }

    Ok(())
}
