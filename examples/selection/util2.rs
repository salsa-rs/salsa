use super::*;

// ANCHOR: util2
#[salsa::query_group(Request)]
trait RequestUtil: RequestParser {
    fn header(&self) -> Vec<ParsedHeader>;
    fn content_type(&self) -> Option<String>;
}

fn header(db: &impl RequestUtil) -> Vec<ParsedHeader> {
    db.parse().header.clone()
}

fn content_type(db: &impl RequestUtil) -> Option<String> {
    db.header()
        .iter()
        .find(|header| header.key == "content-type")
        .map(|header| header.value.clone())
}
// ANCHOR_END: util2
