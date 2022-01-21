# Meta: about the book itself

## Linking policy

We try to avoid links that easily become fragile. 

**Do:**

* Link to `docs.rs` types to document the public API, but modify the link to use `latest` as the version.
* Link to modules in the source code.
* Create ["named anchors"] and embed source code directly.

["named anchors"]: https://rust-lang.github.io/mdBook/format/mdbook.html?highlight=ANCHOR#including-portions-of-a-file

**Don't:**

* Link to direct lines on github, even within a specific commit, unless you are trying to reference a historical piece of code ("how things were at the time").