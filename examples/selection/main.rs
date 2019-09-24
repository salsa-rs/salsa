/// Sources for the [selection pattern chapter][c] of the salsa book.
///
/// [c]: https://salsa-rs.github.io/salsa/common_patterns/selection.html

// ANCHOR: request
#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedResult {
    header: Vec<ParsedHeader>,
    body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedHeader {
    key: String,
    value: String,
}

#[salsa::query_group(Request)]
trait RequestParser {
    /// The base text of the request.
    #[salsa::input]
    fn request_text(&self) -> String;

    /// The parsed form of the request.
    fn parse(&self) -> ParsedResult;
}
// ANCHOR_END: request

fn parse(_db: &impl RequestParser) -> ParsedResult {
    panic!()
}

mod util1;
mod util2;

fn main() {}
