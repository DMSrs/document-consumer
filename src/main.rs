extern crate yaml_rust;
extern crate postgres;

use std::error::Error;

mod config;
use config::Config;

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
use std::fs::File;

use std::io::prelude::*;
use postgres::Connection;
use postgres::TlsMode;
use std::{thread,time};

fn do_loop(conn: &Connection){
    println!("Looping...");
    let query = conn.query("SELECT COUNT(*) FROM documents", &[]);
    if let Ok(res) = query {
        for row in res.iter() {
            println!("We've got {} documents", row.get::<_,i64>(0));
        }
    }
    thread::sleep(time::Duration::from_secs(5));
    do_loop(&conn);
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

    do_loop(&conn.unwrap());
}
