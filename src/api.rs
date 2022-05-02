/// Copyright (c) 2018 by the project authors, as listed in the [AUTHORS](https://github.com/quickwit-oss/tantivy-cli/blob/main/AUTHORS) file.
///
/// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
///
/// The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
///
/// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
/// This tantivy command starts a http server (by default on port 3000)
///
/// Currently the only entrypoint is /api/
/// and it takes the following query string argument
///
/// - `q=` :    your query
///  - `nhits`:  the number of hits that should be returned. (default to 10)
///
///
/// For instance, the following call should return the 20 most relevant
/// hits for fulmicoton.
///
///     http://localhost:3000/api/?q=fulmicoton&nhits=20
///
use iron::mime::Mime;
use iron::prelude::*;
use iron::status;
use iron::typemap::Key;
use mount::Mount;
use persistent::Read;
use serde_derive::Serialize;
use std::convert::From;
use std::error::Error;
use std::fmt::{self, Debug};
use std::str::FromStr;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::QueryParser;
use tantivy::schema::Field;
use tantivy::schema::FieldType;
use tantivy::schema::Schema;
use tantivy::Document;
use tantivy::Index;
use tantivy::IndexReader;
use urlencoded::UrlEncodedQuery;

#[derive(Serialize)]
struct Results {
    items: Vec<AlfredItem>,
}

#[derive(Serialize)]
struct AlfredAction {
    file: Option<String>,
    url: Option<String>,
}

#[derive(Serialize)]
struct AlfredItem {
    title: String,
    arg: String,
    action: AlfredAction,
}

struct IndexServer {
    reader: IndexReader,
    query_parser: QueryParser,
    schema: Schema,
}

impl IndexServer {
    fn load(index: Index) -> tantivy::Result<IndexServer> {
        let schema = index.schema();
        let default_fields: Vec<Field> = schema
            .fields()
            .filter(|&(_, field_entry)| match field_entry.field_type() {
                FieldType::Str(ref text_field_options) => {
                    text_field_options.get_indexing_options().is_some()
                }
                _ => false,
            })
            .map(|(field, _)| field)
            .collect();
        let query_parser =
            QueryParser::new(schema.clone(), default_fields, index.tokenizers().clone());
        let reader = index.reader()?;
        Ok(IndexServer {
            reader,
            query_parser,
            schema,
        })
    }

    fn search(&self, q: String, num_hits: usize, offset: usize) -> tantivy::Result<Results> {
        let query = self
            .query_parser
            .parse_query(&q)
            .expect("Parsing the query failed");
        let searcher = self.reader.searcher();
        let (top_docs, _) = {
            searcher.search(
                &query,
                &(TopDocs::with_limit(num_hits).and_offset(offset), Count),
            )?
        };
        let title = self.schema.get_field("title").unwrap();
        let loc = self.schema.get_field("loc").unwrap();
        let archive_loc = self.schema.get_field("archive_loc").unwrap();
        let items: Vec<AlfredItem> = {
            top_docs
                .iter()
                .map(|(_, doc_address)| {
                    let doc: Document = searcher.doc(*doc_address).unwrap();
                    let aloc = doc.get_first(archive_loc).unwrap().as_text().unwrap();
                    let loc = doc.get_first(loc).unwrap().as_text().unwrap();
                    let location = if aloc.len() == 0 {
                        loc.to_owned()
                    } else {
                        aloc.to_owned()
                    };
                    AlfredItem {
                        title: doc.get_first(title).unwrap().as_text().unwrap().to_owned(),
                        action: AlfredAction {
                            file: if location.starts_with("/") {
                                Some(location.clone())
                            } else {
                                None
                            },
                            url: if location.contains("://") {
                                Some(location.clone())
                            } else {
                                None
                            },
                        },
                        arg: location,
                    }
                })
                .collect()
        };
        Ok(Results { items })
    }
}

impl Key for IndexServer {
    type Value = IndexServer;
}

#[derive(Debug)]
struct StringError(String);

impl fmt::Display for StringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

impl Error for StringError {
    fn description(&self) -> &str {
        &*self.0
    }
}

fn search(req: &mut Request<'_, '_>) -> IronResult<Response> {
    let index_server = req.get::<Read<IndexServer>>().unwrap();
    req.get_ref::<UrlEncodedQuery>()
        .map_err(|_| {
            IronError::new(
                StringError(String::from("Failed to decode error")),
                status::BadRequest,
            )
        })
        .and_then(|ref qs_map| {
            let num_hits: usize = qs_map
                .get("nhits")
                .and_then(|nhits_str| usize::from_str(&nhits_str[0]).ok())
                .unwrap_or(10);
            let query = qs_map.get("q").ok_or_else(|| {
                IronError::new(
                    StringError(String::from("Parameter q is missing from the query")),
                    status::BadRequest,
                )
            })?[0]
                .clone();
            let offset: usize = qs_map
                .get("offset")
                .and_then(|offset_str| usize::from_str(&offset_str[0]).ok())
                .unwrap_or(0);
            let serp = index_server.search(query, num_hits, offset).unwrap();
            let resp_json = serde_json::to_string_pretty(&serp).unwrap();
            let content_type = "application/json".parse::<Mime>().unwrap();
            Ok(Response::with((
                content_type,
                status::Ok,
                format!("{}", resp_json),
            )))
        })
}

pub fn serve(index: Index, host: &str) -> tantivy::Result<()> {
    let mut mount = Mount::new();
    let server = IndexServer::load(index)?;

    mount.mount("/api", search);

    let mut middleware = Chain::new(mount);
    middleware.link(Read::<IndexServer>::both(server));

    println!("listening on http://{}", host);
    Iron::new(middleware).http(host).unwrap();
    Ok(())
}
