use std::default::Default;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::sync_channel;
use std::thread;

use anyhow::Result;
use ignore::{WalkBuilder, WalkState};
use serde::Deserialize;
use tantivy::{directory::MmapDirectory, doc, schema::*, Index};

pub mod api;

#[derive(Debug)]
pub struct Entry {
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

/// Digest the file at the provided path, producing an entry
/// when possible.
fn digest(path: String) -> Result<Option<Entry>> {
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

pub fn build_index(src_path: String, db_path: Option<String>, threads: usize) -> Result<Index> {
    let mut schema_builder = Schema::builder();
    let title = schema_builder.add_text_field("title", TEXT | STORED);
    let body = schema_builder.add_text_field("body", TEXT);
    let loc = schema_builder.add_text_field("loc", STRING | STORED);
    let archive_loc = schema_builder.add_text_field("archive_loc", STRING | STORED);
    let schema = schema_builder.build();
    let index = match db_path {
        Some(path) => Index::open_or_create(MmapDirectory::open(path)?, schema)?,
        None => Index::create_in_ram(schema),
    };

    let (tx, rx) = sync_channel::<Entry>(threads);

    let mut index_writer = index.writer_with_num_threads(threads, 50_000_000).unwrap();
    let writer = thread::spawn(move || {
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

    let walker = WalkBuilder::new(src_path)
        .follow_links(false)
        .threads(threads)
        .build_parallel();

    walker.run(|| {
        let tx = tx.clone();
        Box::new(move |entry_o| match entry_o {
            Ok(de) => {
                if de.path().is_file() {
                    let path = de.into_path().into_os_string().into_string().unwrap();
                    match digest(path) {
                        Ok(Some(entry)) => {
                            tx.send(entry).unwrap();
                        }
                        Ok(None) => {}
                        Err(e) => eprintln!("{}", e),
                    };
                }
                return WalkState::Continue;
            }
            Err(e) => {
                eprintln!("failed to read: {}", e);
                WalkState::Quit
            }
        })
    });
    drop(tx);
    writer.join().unwrap();
    Ok(index)
}
