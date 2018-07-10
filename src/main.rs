extern crate yaml_rust;
extern crate postgres;
extern crate fwatcher;
extern crate sha2;
extern crate hex;
extern crate poppler;
extern crate tesseract;
extern crate glob;


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
use std::process::Command;
use std::fs;
use sha2::Digest;
use hex::ToHex;
use tesseract::Tesseract;
use glob::glob;

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

fn cleanup(path: &PathBuf){
    println!("Removing original file from {:?} ...", path);
    let _ = fs::remove_file(path);
}

fn parse_document(_conn: &Connection, path: &PathBuf){
    println!("Parsing document {:?}", path);

    if !path.exists() {
        println!("Provided path doesn't exists.");
        return;
    }

    if !path.is_file() {
        println!("Provided path is not a file!");
        return;
    }

    let _ocr_text : Vec<String> = Vec::new();

    // Calculate SHA256 sum (save as HEX)

    let mut sha256 = sha2::Sha256::default();
    let mut f = File::open(path).expect("Unable to open this file.");
    let mut buffer = Vec::new();
    let _ = f.read_to_end(&mut buffer);
    sha256.input(buffer.as_slice());

    let mut sha256_bytes : [u8; 32] = Default::default();
    sha256_bytes.copy_from_slice(&sha256.result()[..32]);

    let mut sha256_hex : String = String::new();
    sha256_bytes.write_hex(&mut sha256_hex).expect("Unable to write HEX");

    println!("SHA256 Sum: {}", sha256_hex);

    // TODO: Implement!
    /*  Step 0:  Use poppler to check if the document has any text on it,
                if this is the case, ignore the OCR part and just store
                the document w/ the OCR field set as the document page text.
        =====================================================================
    */

    //  Step 1: Convert document from PDF to PNG
    //  =====================================================================


    // TODO: Improve w/ libpoppler.
    // Convert document to images (to use tesseract-rs)
    let mut child = Command::new("pdftoppm")
        .arg(path)
        .arg("-r")
        .arg("300")
        .arg(format!("tmp/{}",sha256_hex))
        .arg("-png")
        .spawn().expect("Unable to start pdftoppm");
    let exit_code = child.wait().expect("Execution failed");

    if !exit_code.success() {
        // Unable to process document!
        return;
    }

    /*  Step 2:  OCR the generated files, store the OCR result
                in a Vec<String>, so that we have a
                page => text association.
        =====================================================================
    */

    let mut pages_text : Vec<String> = Vec::new();

    let tesseract = Tesseract::new();
    for entry in glob(&format!("tmp/{}*.png", sha256_hex)).unwrap() {
        match entry {
            Ok(path) => {
                println!("Parsing {:?}", path);
                tesseract.set_lang("ita");
                tesseract.set_image(path.to_str().unwrap());
                let recognized_text = tesseract.get_text();
                &mut pages_text.push(String::from(recognized_text));
            }

            Err(e) => println!("Globbing: Pattern matched but unreadable! {:?}", e)
        }
    }

    for el in pages_text.iter() {
        println!("Recognized text: {}", el);
    }

    /*  Step 3: Send everything to the backend, move the document to the
                stored documents, {id}.pdf
    */

    cleanup(&path);
}

fn document_change(conn: &Connection, event: &DebouncedEvent){
    match event {
        DebouncedEvent::Create(p) => {
            parse_document(conn, &p);
        }
        _ => {
            println!("Event not parsed: {:?}", event);
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
    .pattern(Pattern::new("*.pdf").unwrap())
    .interval(Duration::new(1, 0))
    .restart(false)
    .run();
}
