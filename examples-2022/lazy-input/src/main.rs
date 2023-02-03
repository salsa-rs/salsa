use std::{path::PathBuf, sync::Mutex, time::Duration};

use crossbeam_channel::{unbounded, Sender};
use dashmap::{mapref::entry::Entry, DashMap};
use eyre::{eyre, Context, Report, Result};
use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    DebounceEventResult, Debouncer,
};
use salsa::DebugWithDb;

// ANCHOR: main
fn main() -> Result<()> {
    // Create the channel to receive file change events.
    let (tx, rx) = unbounded();
    let mut db = Database::new(tx);

    let initial_file_path: PathBuf = std::env::args_os()
        .nth(1)
        .ok_or_else(|| eyre!("Usage: ./lazy-input <input-file>"))?
        .into();

    loop {
        // Compile the code starting at the provided input, this will read other
        // needed files using the on-demand mechanism.
        let file = db.file(initial_file_path.clone())?;
        let sum = sum(&db, file);
        let diagnostics = sum::accumulated::<Diagnostic>(&db, file);
        if diagnostics.is_empty() {
            println!("Sum is: {}", sum);
        } else {
            for diagnostic in diagnostics {
                println!("{}", diagnostic);
            }
        }

        for log in db.logs.lock().unwrap().drain(..) {
            eprintln!("{}", log);
        }

        // Wait for file change events, the output can't change unless the
        // inputs change.
        for event in rx.recv()?.unwrap() {
            let path = event.path.canonicalize().wrap_err_with(|| {
                format!("Failed to canonicalize path {}", event.path.display())
            })?;
            let contents = match db.files.get(&path) {
                Some(contents) => *contents,
                None => continue,
            };
            // `path` has changed, so read it and update the contents to match.
            // This creates a new revision and causes the incremental algorithm
            // to kick in, just like any other update to a salsa input.
            let data = std::fs::read_to_string(path)
                .wrap_err_with(|| format!("Failed to read file {}", event.path.display()))?;
            contents.set_data(&mut db).to(data);
        }
    }
}
// ANCHOR_END: main

#[salsa::jar(db = Db)]
struct Jar(Diagnostic, File, Contents, ParsedFile, parse, sum);

#[salsa::interned]
struct File {
    #[return_ref]
    path: PathBuf,
}

// ANCHOR: db
#[salsa::input]
struct Contents {
    #[return_ref]
    data: String,
}

trait Db: salsa::DbWithJar<Jar> {
    fn file(&self, path: PathBuf) -> Result<File>;
    fn contents(&self, file: File) -> Result<&str>;
}

#[salsa::db(Jar)]
struct Database {
    storage: salsa::Storage<Self>,
    logs: Mutex<Vec<String>>,
    files: DashMap<PathBuf, Contents>,
    file_watcher: Mutex<Debouncer<RecommendedWatcher>>,
}

impl Database {
    fn new(tx: Sender<DebounceEventResult>) -> Self {
        let storage = Default::default();
        Self {
            storage,
            logs: Default::default(),
            files: DashMap::new(),
            file_watcher: Mutex::new(new_debouncer(Duration::from_secs(1), None, tx).unwrap()),
        }
    }
}

impl Db for Database {
    fn file(&self, path: PathBuf) -> Result<File> {
        let path = path
            .canonicalize()
            .wrap_err_with(|| format!("Failed to read {}", path.display()))?;
        Ok(File::new(self, path))
    }

    fn contents(&self, file: File) -> Result<&str> {
        let contents = match self.files.entry(file.path(self).clone()) {
            // If the file already exists in our cache then just return it.
            Entry::Occupied(entry) => *entry.get(),
            // If we haven't read this file yet set up the watch, read the
            // contents, store it in the cache, and return it.
            Entry::Vacant(entry) => {
                // Set up the watch before reading the contents to try to avoid
                // race conditions.
                let path = file.path(self);
                let watcher = &mut *self.file_watcher.lock().unwrap();
                watcher
                    .watcher()
                    .watch(path, RecursiveMode::NonRecursive)
                    .unwrap();
                let data = std::fs::read_to_string(path)
                    .wrap_err_with(|| format!("Failed to read {}", path.display()))?;
                *entry.insert(Contents::new(self, data))
            }
        };
        Ok(contents.data(self))
    }
}
// ANCHOR_END: db

impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        // don't log boring events
        if let salsa::EventKind::WillExecute { .. } = event.kind {
            self.logs
                .lock()
                .unwrap()
                .push(format!("{:?}", event.debug(self)));
        }
    }
}

#[salsa::accumulator]
struct Diagnostic(String);

impl Diagnostic {
    fn push_error(db: &dyn Db, file: File, error: Report) {
        Diagnostic::push(
            db,
            format!(
                "Error in file {}: {:?}\n",
                file.path(db)
                    .file_name()
                    .unwrap_or_else(|| "<unknown>".as_ref())
                    .to_string_lossy(),
                error,
            ),
        )
    }
}

#[salsa::tracked]
struct ParsedFile {
    value: u32,
    #[return_ref]
    links: Vec<File>,
}

#[salsa::tracked]
fn parse(db: &dyn Db, file: File) -> ParsedFile {
    let contents = match db.contents(file) {
        Ok(file) => file,
        Err(err) => {
            Diagnostic::push_error(db, file, err);
            return ParsedFile::new(db, 0, vec![]);
        }
    };

    let mut lines = contents.lines();
    let value = match lines.next().map(|line| (line.parse::<u32>(), line)) {
        Some((Ok(num), _)) => num,
        Some((Err(e), line)) => {
            Diagnostic::push_error(
                db,
                file,
                Report::new(e).wrap_err(format!(
                    "First line ({}) could not be parsed as an integer",
                    line
                )),
            );
            0
        }
        None => {
            Diagnostic::push_error(db, file, eyre!("File must contain an integer"));
            0
        }
    };
    let links = lines
        .filter_map(|path| {
            let relative_path = match path.parse::<PathBuf>() {
                Ok(path) => path,
                Err(err) => {
                    Diagnostic::push_error(
                        db,
                        file,
                        Report::new(err).wrap_err(format!("Failed to parse path: {}", path)),
                    );
                    return None;
                }
            };
            let path = file.path(db).parent().unwrap().join(relative_path);
            db.file(path)
                .map_err(|err| {
                    Diagnostic::push_error(db, file, err);
                })
                .ok()
        })
        .collect();
    ParsedFile::new(db, value, links)
}

#[salsa::tracked(recovery_fn = recover_sum)]
fn sum(db: &dyn Db, file: File) -> u32 {
    let parsed = parse(db, file);
    parsed.value(db)
        + parsed
            .links(db)
            .iter()
            .map(|&file| sum(db, file))
            .sum::<u32>()
}

fn recover_sum(db: &dyn Db, _cycle: &salsa::Cycle, file: File) -> u32 {
    Diagnostic::push_error(db, file, Report::msg("File cycle"));
    0
}
