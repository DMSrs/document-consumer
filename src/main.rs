extern crate fwatcher;
extern crate glob;
extern crate hex;
extern crate poppler;
extern crate postgres;
extern crate regex;
extern crate sha2;
extern crate tesseract;
extern crate chrono;
extern crate ansi_term;

#[macro_use] extern crate log;

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
use log::{Record, Level, Metadata};
use ansi_term::{Colour, Style};

use std::result::Result;
use whatlang::Lang;
use whatlang::Detector;
use std::process;
use log::LevelFilter;

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

                info!("Page number: {}", page_nr);
                // TODO: HashMap, maybe?

                tesseract.set_lang(&config.ocr.lang);
                tesseract.set_image(path.to_str().unwrap());
                let recognized_text = tesseract.get_text();
                &mut pages_text.push(String::from(recognized_text));
                let _ = fs::remove_file(path);
            }

            _ => warn!("Globbing: Pattern matched but unreadable!"),
        }
    }

    for el in pages_text.iter() {
        info!("Recognized text: {}", el);
    }

    Ok(pages_text)
}

fn parse_document(conn: &Connection, config: &Config, path: &PathBuf) {
    info!("Parsing document {:?}", path);

    if !path.exists() {
        warn!("Provided path doesn't exists.");
        return;
    }

    if !path.is_file() {
        warn!("Provided path is not a file!");
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

    info!("SHA256 Sum: {}", sha256_hex);

    // TODO: Implement!
    /*  Step 0:  Use poppler to check if the document has any text on it,
                if this is the case, ignore the OCR part and just store
                the document w/ the OCR field set as the document page text.
        =====================================================================
    */

    let pd = poppler::PopplerDocument::new_from_file(path, "")
        .expect("Unable to open document");

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
        info!("Document is empty!");
        pages_text = match perform_ocr(&config,  &sha256_hex,&path) {
            Ok(pt) => {
                pt
            }

            Err(e) => {
                error!("Unable to perform OCR, error was {}", e);
                Vec::new()
            }
        }
    }

    let detector = Detector::new();
    let mut languages_detected : Vec<Option<Lang>> = Vec::new();

    for i in &pages_text {
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
    match conn.query("INSERT INTO documents (correspondent, title, added_on, date, sha256sum) VALUES (
                NULL,
                $1,
                $2,
                $3,
                $4) RETURNING id;", &[
                &today_date.format("%Y-%m-%d").to_string(),
                &today_date.naive_utc(),
                &today_date.naive_utc(),
                &sha256_hex
    ]) {
        Ok(r) => {
            let doc_id : i32 = r.get(0).get(0);
            info!("ID {} inserted!", doc_id);

            let pdf_path = PathBuf::from(format!("{}/pdf", config.paths.data));
            if !&pdf_path.exists() {
                warn!("PDF dir doesn't exist, creating it now...");
                fs::create_dir(&pdf_path).expect("Unable to create PDF dir");
            }

            conn.execute("INSERT INTO tags_documents (tag_slug, document_id) VALUES
                ($1, $2)", &[&"untagged", &doc_id]).expect("Unable to set document tag");

            let new_path = PathBuf::from(format!("{}/{}.pdf", pdf_path.to_str().unwrap(), doc_id));
            fs::rename(path, new_path).expect("Unable to move parsed file!");

            let mut pn = 1;
            for i in &pages_text {
                match conn.execute("INSERT INTO pages (document_id, text, tsv, number) VALUES (\
                    $1,\
                    $2,
                    to_tsvector('english', $2),\
                    $3);", &[
                    &doc_id,
                    &i,
                    &pn
                ]) {
                    Ok(_r) => {
                        println!("Page {} inserted succesfully!", &pn);
                    }

                    Err(e) => {
                        println!("Error while adding {}: {}", &pn, e);
                    }
                }

                pn = pn + 1;
            }
            cleanup(&path);
        },

        Err(e) => {
            error!("Unable to add the document to DB. Error was {}", e);
        }
    }
}

fn document_change(conn: &Connection, config: &Config, event: &DebouncedEvent) {
    match event {
        DebouncedEvent::Create(p) => {
            parse_document(conn, &config, &p);
        }
        _ => {
            info!("Event not parsed: {:?}", event);
        }
    }
}

struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let style : Style = match record.metadata().level() {
                Level::Error => {
                    Style::new().bold().fg(Colour::Red)
                },
                Level::Warn => {
                    Style::new().bold().fg(Colour::Red)
                },
                _ => {
                    Style::new().fg(Colour::Blue)
                }
            };

            let input_string = format!("{}", record.args());
            println!("{}", style.paint(input_string));
        }
    }

    fn flush(&self) {}
}

static LOGGER: SimpleLogger = SimpleLogger;

fn main() {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(LevelFilter::Info))
        .expect("Unable to initialize logger");
    let cfg: Config = load_config().unwrap_or(Config::new());

    info!("Hostname: {}", cfg.db.hostname);
    info!("Username: {}", cfg.db.username);
    info!("Password: {}", cfg.db.password);
    info!("OCR Language: {}", cfg.ocr.lang);
    info!("OCR DPI: {}", cfg.ocr.dpi);
    info!("Data Path: {}", cfg.paths.data);
    info!("Consumption Path: {}", cfg.paths.consumption);

    // Check that data_path exists and is a directory
    let path = PathBuf::from(&cfg.paths.data);
    if !path.is_dir() {
        error!("{} is an invalid data directory.", &cfg.paths.data);
        process::exit(1);
    }

    // Check that the consumption dir exists and it's a directory
    let path = PathBuf::from(&cfg.paths.consumption);
    if !path.is_dir() {
        error!("{} is an invalid consumption directory.", &cfg.paths.consumption);
        process::exit(2);
    }

    let conn = Connection::connect(
        format!(
            "postgres://{}:{}@{}:5432",
            cfg.db.username, cfg.db.password, cfg.db.hostname
        ),
        TlsMode::None,
    );

    if let Err(e) = conn {
        error!("Unable to connect to DB. Error was {}", e.description());
        return;
    }

    info!("DB Connection successful!");

    let dirs = vec![PathBuf::from(&cfg.paths.consumption)];

    let c = conn.unwrap();

    Fwatcher::<Box<Fn(&DebouncedEvent)>>::new(
        dirs,
        Box::new(move |e| {
            document_change(&c, &cfg, e);
        }),
    )
    
    .pattern(Pattern::new("*.pdf").unwrap())
    .interval(Duration::new(0, 0))
    .restart(true)
    .run();
}
