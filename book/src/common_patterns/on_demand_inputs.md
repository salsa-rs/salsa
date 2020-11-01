# On-Demand (Lazy) Inputs

Salsa input queries work best if you can easily provide all of the inputs upfront.
However sometimes the set of inputs is not known beforehand.

A typical example is reading files from disk.
While it is possible to eagerly scan a particular directory and create an in-memory file tree in a salsa input query, a more straight-forward approach is to read the files lazily.
That is, when someone requests the text of a file for the first time:

1. Read the file from disk and cache it.
2. Setup a file-system watcher for this path.
3. Invalidate the cached file once the watcher sends a change notification.

This is possible to achieve in salsa, using a derived query and `report_synthetic_read` and `invalidate` queries.
The setup looks roughly like this:

```rust,ignore
#[salsa::query_group(VfsDatabaseStorage)]
trait VfsDatabase: salsa::Database + FileWatcher {
    fn read(&self, path: PathBuf) -> String;
}

trait FileWatcher {
    fn watch(&self, path: &Path);
    fn did_change_file(&mut self, path: &Path);
}

fn read(db: &dyn salsa::Database, path: PathBuf) -> String {
    db.salsa_runtime()
        .report_synthetic_read(salsa::Durability::LOW);
    db.watch(&path);
    std::fs::read_to_string(&path).unwrap_or_default()
}

#[salsa::database(VfsDatabaseStorage)]
struct MyDatabase { ... }

impl FileWatcher for MyDatabase {
    fn watch(&self, path: &Path) { ... }
    fn did_change_file(&mut self, path: &Path) {
        ReadQuery.in_db_mut(self).invalidate(path);
    }
}
```

- We declare the query as a derived query (which is the default).
- In the query implementation, we don't call any other query and just directly read file from disk.
- Because the query doesn't read any inputs, it will be assigned a `HIGH` durability by default, which we override with `report_synthetic_read`.
- The result of the query is cached, and we must call `invalidate` to clear this cache.

A complete, runnable file-watching example can be found in [this git repo](https://github.com/ChristopherBiscardi/salsa-file-watch-example/blob/f968dc8ea13a90373f91d962f173de3fe6ae24cd/main.rs) along with [a write-up](https://www.christopherbiscardi.com/on-demand-lazy-inputs-for-incremental-computation-in-salsa-with-file-watching-powered-by-notify-in-rust) that explains more about the code and what it is doing.
