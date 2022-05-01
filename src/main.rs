use std::default::Default;
use std::fs::File;
use std::io::{self, BufRead, Read};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::sync_channel;
use std::thread;

use anyhow::Result;
use serde::Deserialize;
use tantivy::{directory::MmapDirectory, doc, schema::*, Index};
use threadpool::ThreadPool;

#[derive(Debug)]
struct Entry {
    pub title: String,
    pub body: Option<String>,
    pub loc: Option<String>,
    pub archive_loc: Option<String>,
}

#[derive(Deserialize)]
struct Webloc {
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "URL")]
    pub url: String,
}

fn handle_webloc(path: String) -> Result<Option<Entry>> {
    let webloc: Webloc = plist::from_file(&path)?;
    Ok(Some(Entry {
        title: webloc.name.unwrap_or_else(|| filename(&Path::new(&path))),
        loc: Some(webloc.url),
        body: None,
        archive_loc: None,
    }))
}

fn handle_text(path: String) -> Result<Option<Entry>> {
    let mut contents = String::new();
    let mut file = File::open(&path)?;
    file.read_to_string(&mut contents)?;
    let p = Path::new(&path);
    Ok(Some(Entry {
        title: filename(&p),
        loc: None,
        body: Some(contents),
        archive_loc: Some(path),
    }))
}

// Jesus christ.
fn filename(p: &Path) -> String {
    p.file_stem().unwrap().to_owned().into_string().unwrap()
}

fn handle_pdf(path: String) -> Result<Option<Entry>> {
    let p = Path::new(&path);
    let mut tmp_path = std::env::temp_dir();
    tmp_path.push(filename(&p));
    let _ = Command::new("pdftotext")
        .args([&path, tmp_path.to_str().unwrap()])
        .output()?;

    let mut contents = String::new();
    let mut file = File::open(&tmp_path)?;
    file.read_to_string(&mut contents)?;

    drop(file);
    std::fs::remove_file(&tmp_path)?;

    Ok(Some(Entry {
        title: filename(&p),
        loc: None,
        body: Some(contents),
        archive_loc: Some(path),
    }))
}

fn process(path: String) -> Result<Option<Entry>> {
    if let Some((_, extension)) = path.rsplit_once(".") {
        match extension {
            "pdf" => handle_pdf(path),
            "txt" | "markdown" | "md" => handle_text(path),
            "webloc" => handle_webloc(path),
            _ => Ok(None),
        }
    } else {
        Ok(Some(Entry {
            title: filename(&Path::new(&path)),
            archive_loc: Some(path),
            body: None,
            loc: None,
        }))
    }
}

fn main() -> Result<()> {
    let mut schema_builder = Schema::builder();
    let title = schema_builder.add_text_field("title", TEXT | STORED);
    let body = schema_builder.add_text_field("body", TEXT);
    let loc = schema_builder.add_text_field("loc", STRING);
    let archive_loc = schema_builder.add_text_field("archive_loc", STRING);
    let schema = schema_builder.build();

    let index = Index::create(MmapDirectory::open("db")?, schema, Default::default())?;
    let (tx, rx) = sync_channel(100);

    let stdin = io::stdin(); // We get `Stdin` here.
    let pool = ThreadPool::new(8);

    for res in stdin.lock().lines() {
        let tx = tx.clone();
        match res {
            Ok(line) => pool.execute(move || {
                match process(line) {
                    Ok(Some(entry)) => {
                        tx.send(entry).unwrap();
                    }
                    Ok(None) => return,
                    Err(e) => eprintln!("{}", e),
                };
            }),
            Err(e) => eprintln!("failed to read: {}", e),
        };
    }
    drop(tx);
    let writer = thread::spawn(move || {
        let mut index_writer = index.writer_with_num_threads(8, 50_000_000).unwrap();
        for entry in rx.iter() {
            index_writer
                .add_document(doc!(
                    title => entry.title,
                    body => entry.body.unwrap_or("".to_string()),
                    loc => entry.loc.unwrap_or("".to_string()),
                    archive_loc => entry.archive_loc.unwrap_or("".to_string()),
                ))
                .expect("failed to insert doc");
        }
        index_writer.commit().expect("failed to commit");
    });
    pool.join();
    writer.join().unwrap();
    Ok(())
}
