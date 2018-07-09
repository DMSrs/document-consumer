extern crate yaml_rust;
extern crate postgres;
extern crate fwatcher;

use std::error::Error;

mod config;
use config::Config;

use std::fs::File;

use std::io::prelude::*;
use postgres::Connection;
use postgres::TlsMode;
use std::time::Duration;
use std::path::PathBuf;

use fwatcher::Fwatcher;
use fwatcher::glob::Pattern;
use fwatcher::notify::DebouncedEvent;

use yaml_rust::{YamlLoader};
fn load_config() -> Option<Config> {
    if let Ok(mut f) = File::open("./config.yml") {
        let mut content = String::new();
        if let Err(e) = f.read_to_string(&mut content) {
            println!("Error: Unable to read from config file! {}", e.description());
            return None;
        }

        if let Ok(yaml) = YamlLoader::load_from_str(&content) {
            if !yaml.is_empty(){
                let doc = &yaml[0];
                let config = &doc["config"];
                if !&config.is_badvalue() {
                    let db_hostname : String = (&config["db_hostname"])
                        .as_str().unwrap_or("hostname").to_string();
                    let db_username : String = (&config["db_username"])
                        .as_str().unwrap_or("postgres").to_string();
                    let db_password : String = (&config["db_password"])
                        .as_str().unwrap_or("default").to_string();

                    return Some(Config {
                        db_hostname,
                        db_username,
                        db_password
                    });
                }
            }
        }
    }

    None
}

fn parse_document(conn: &Connection, path: &PathBuf){
    println!("Parsing document {:?}", path);
}

fn document_change(conn: &Connection, event: &DebouncedEvent){
    match event {
        DebouncedEvent::Create(p) => {
            println!("Created {:?}", p);
            parse_document(conn, p);
        }
        _ => {

        }
    }
}

fn main() {
    let cfg : Config = load_config().unwrap_or(Config {
        db_hostname: String::new(),
        db_username: String::new(),
        db_password: String::new(),
    });

    println!("Hostname: {}", cfg.db_hostname);
    println!("Username: {}", cfg.db_username);
    println!("Password: {}", cfg.db_password);

    let conn = Connection::connect(
        format!("postgres://{}:{}@{}:5432",
                cfg.db_username,
                cfg.db_password,
                cfg.db_hostname
        ), TlsMode::None);

    if let Err(e) = conn {
        println!("Unable to connect to DB. Error was {}", e.description());
        return;
    }

    println!("DB Connection successful!");

    let dirs = vec![PathBuf::from("consumption-dir/")];

    let c = conn.unwrap();

    Fwatcher::<Box<Fn(&DebouncedEvent)>>::new(dirs, Box::new(move |e|
    {
        document_change(&c, e);
    }))
    .pattern(Pattern::new("**/*.pdf").unwrap())
    .interval(Duration::new(1, 0))
    .restart(false)
    .run();
}
