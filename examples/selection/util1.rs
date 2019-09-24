use super::*;

// ANCHOR: util1
#[salsa::query_group(Request)]
trait RequestUtil: RequestParser {
    fn content_type(&self) -> Option<String>;
}

fn content_type(db: &impl RequestUtil) -> Option<String> {
    db.parse()
        .header
        .iter()
        .find(|header| header.key == "content-type")
        .map(|header| header.value.clone())
}
// ANCHOR_END: util1
