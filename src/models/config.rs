#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub db: DbConfig,
    pub ocr : OcrConfig
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DbConfig {
    pub hostname : String,
    pub username : String,
    pub password : String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OcrConfig {
    pub lang: String,
    pub dpi: i32
}

impl Config {
    pub fn new() -> Config {
        Config {
            db: DbConfig::new(),
            ocr: OcrConfig::new(),
        }
    }
}

impl DbConfig {
    pub fn new() -> DbConfig {
        DbConfig {
            hostname: String::new(),
            username: String::new(),
            password: String::new(),
        }
    }
}

impl OcrConfig {
    pub fn new() -> OcrConfig {
        OcrConfig {
            lang: String::from("eng"),
            dpi: 300,
        }
    }
}