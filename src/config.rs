#[derive(Debug, Deserialize)]
pub struct Config {
    pub editor: String,
}

impl Config {
    pub fn default() -> Self {
        Self {
            editor: String::from("vim"),
        }
    }
}