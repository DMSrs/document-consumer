extern crate fwatcher;
extern crate glob;
extern crate hex;
extern crate poppler;
extern crate postgres;
extern crate regex;
extern crate sha2;
extern crate tesseract;
extern crate chrono;

#[macro_use]
extern crate serde_derive;
extern crate serde_yaml;
extern crate whatlang;

use std::error::Error;

mod models;
use models::config::Config;

use std::fs::File;

use postgres::Connection;
use postgres::TlsMode;
use std::io::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

use fwatcher::glob::Pattern;
use fwatcher::notify::DebouncedEvent;
use fwatcher::Fwatcher;

use glob::{glob, Paths};
use hex::ToHex;
use regex::Regex;
use sha2::Digest;
use std::fs;
use std::process::Command;
use tesseract::Tesseract;
use chrono::prelude::*;

use std::result::Result;
use whatlang::{detect, Lang, Script};
use whatlang::Detector;

fn load_config() -> Option<Config> {
    if let Ok(mut f) = File::open("./config.yml") {
        let mut content = String::new();
        if let Err(e) = f.read_to_string(&mut content) {
            println!(
                "Error: Unable to read from config file! {}",
                e.description()
            );
            return None;
        }
        return Some(serde_yaml::from_str(&content).unwrap_or(Config::new()));
    }
    None
}

fn cleanup(path: &PathBuf) {
    println!("Removing original file from {:?} ...", path);
    let _ = fs::remove_file(path);
}

fn perform_ocr(config: &Config, sha256_hex: &str, path: &PathBuf) -> Result<Vec<String>, &'static str> {

    let mut pages_text : Vec<String> = Vec::new();

    //  Step 1: Convert document from PDF to PNG
    //  =====================================================================

    // TODO: Improve w/ libpoppler.
    // Convert document to images (to use tesseract-rs)
    let mut child = Command::new("pdftoppm")
        .arg(path)
        .arg("-r")
        .arg(config.ocr.dpi.to_string())
        .arg(format!("tmp/{}", sha256_hex))
        .arg("-png")
        .spawn()
        .expect("Unable to start pdftoppm");
    let exit_code = child.wait().expect("Execution failed");

    if !exit_code.success() {
        // Unable to process document!
        return Err("Unable to convert from PDF to PNG");
    }

    /*  Step 2:  OCR the generated files, store the OCR result
                in a Vec<String>, so that we have a
                page => text association.
        =====================================================================
    */

    let tesseract = Tesseract::new();
    let paths: Paths = glob(&format!("tmp/{}*.png", sha256_hex)).unwrap();
    for entry in paths {
        match entry {
            Ok(path) => {
                let file_name: String = String::from(path.file_name().unwrap().to_str().unwrap());
                let re = Regex::new(r"^.*-(\d+)\.png$").unwrap();
                if !re.is_match(&file_name) {
                    println!("Regex unmatched");
                    break;
                }

                let cap = re.captures(&file_name).unwrap();
                let page_nr = (&cap[1]).parse::<i32>().unwrap();

                println!("Page number: {}", page_nr);
                // TODO: HashMap, maybe?

                tesseract.set_lang(&config.ocr.lang);
                tesseract.set_image(path.to_str().unwrap());
                let recognized_text = tesseract.get_text();
                &mut pages_text.push(String::from(recognized_text));
                let _ = fs::remove_file(path);
            }

            _ => println!("Globbing: Pattern matched but unreadable!"),
        }
    }

    for el in pages_text.iter() {
        println!("Recognized text: {}", el);
    }

    Ok(pages_text)
}

fn parse_document(conn: &Connection, config: &Config, path: &PathBuf) {
    println!("Parsing document {:?}", path);

    if !path.exists() {
        println!("Provided path doesn't exists.");
        return;
    }

    if !path.is_file() {
        println!("Provided path is not a file!");
        return;
    }

    let mut pages_text: Vec<String> = Vec::new();

    // Calculate SHA256 sum (save as HEX)

    let mut sha256 = sha2::Sha256::default();
    let mut f = File::open(path).expect("Unable to open this file.");
    let mut buffer = Vec::new();
    let _ = f.read_to_end(&mut buffer);
    sha256.input(buffer.as_slice());

    let mut sha256_bytes: [u8; 32] = Default::default();
    sha256_bytes.copy_from_slice(&sha256.result()[..32]);

    let mut sha256_hex: String = String::new();
    sha256_bytes
        .write_hex(&mut sha256_hex)
        .expect("Unable to write HEX");

    println!("SHA256 Sum: {}", sha256_hex);

    // TODO: Implement!
    /*  Step 0:  Use poppler to check if the document has any text on it,
                if this is the case, ignore the OCR part and just store
                the document w/ the OCR field set as the document page text.
        =====================================================================
    */

    let pd = poppler::PopplerDocument::new_from_file(path, "")
        .expect("Document Parsed Correctly");

    let mut doc_empty = true;

    /*  Step 1: Check if the document needs to be OCR'd */

    for i in 0..pd.get_n_pages() {
        let page = pd.get_page(i).unwrap();
        let text = page.get_text().unwrap();
        if !text.is_empty() {
            doc_empty = false;
        }

        pages_text.push(String::from(text));
    }

    if doc_empty {
        /*  Step 1a: The document needs to be OCR'd  - perform OCR */
        pages_text = match perform_ocr(&config,  &sha256_hex,&path) {
            Ok(pt) => {
                pt
            }

            Err(e) => {
                println!("Unable to perform OCR, error was {}", e);
                Vec::new()
            }
        }
    }

    let detector = Detector::new();
    let mut languages_detected : Vec<Option<Lang>> = Vec::new();

    for i in pages_text {
        languages_detected.push(detector.detect_lang(&i));
    }

    for i in languages_detected {
        println!("Language: {:?}", i);
    }

    /*  Step 2: Send everything to the backend, move the document to the
                stored documents, {id}.pdf
    */

    let now : DateTime<Utc> = Utc::now();
    let today_date = now.date();

    // Create a Document (to get an ID!)
    match conn.execute("INSERT INTO documents (correspondent, title, added_on, date, sha256sum) VALUES (
                NULL,
                $1,
                $2,
                $3,
                $4);", &[
                &today_date.format("%Y-%m-%d").to_string(),
                &today_date.naive_utc(),
                &today_date.naive_utc(),
                &sha256_hex
    ]) {
        Ok(r) => {
            println!("{} rows inserted!", r);
            cleanup(&path);
        },

        Err(e) => {
            println!("Unable to add the document to DB. Error was {}", e);
        }
    }
}

fn document_change(conn: &Connection, config: &Config, event: &DebouncedEvent) {
    match event {
        DebouncedEvent::Create(p) => {
            parse_document(conn, &config, &p);
        }
        _ => {
            println!("Event not parsed: {:?}", event);
        }
    }
}

fn main() {
    let cfg: Config = load_config().unwrap_or(Config::new());

    println!("Hostname: {}", cfg.db.hostname);
    println!("Username: {}", cfg.db.username);
    println!("Password: {}", cfg.db.password);
    println!("OCR Language: {}", cfg.ocr.lang);
    println!("OCR DPI: {}", cfg.ocr.dpi);

    let conn = Connection::connect(
        format!(
            "postgres://{}:{}@{}:5432",
            cfg.db.username, cfg.db.password, cfg.db.hostname
        ),
        TlsMode::None,
    );

    if let Err(e) = conn {
        println!("Unable to connect to DB. Error was {}", e.description());
        return;
    }

    println!("DB Connection successful!");

    let dirs = vec![PathBuf::from("consumption-dir/")];

    let c = conn.unwrap();

    Fwatcher::<Box<Fn(&DebouncedEvent)>>::new(
        dirs,
        Box::new(move |e| {
            document_change(&c, &cfg, e);
        }),
    )
    
    .pattern(Pattern::new("*.pdf").unwrap())
    .interval(Duration::new(1, 0))
    .restart(false)
    .run();
}
