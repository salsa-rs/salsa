use std::{path::PathBuf, sync::Mutex, time::Duration};

use crossbeam::channel::{unbounded, Sender};
use dashmap::{mapref::entry::Entry, DashMap};
use eyre::{eyre, Context, Report, Result};
use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    DebounceEventResult, Debouncer,
};
use salsa::{Accumulator, Setter, Storage};

// ANCHOR: main
fn main() -> Result<()> {
    // Create the channel to receive file change events.
    let (tx, rx) = unbounded();
    let mut db = LazyInputDatabase::new(tx);

    let initial_file_path = std::env::args_os()
        .nth(1)
        .ok_or_else(|| eyre!("Usage: ./lazy-input <input-file>"))?;

    // Create the initial input using the input method so that changes to it
    // will be watched like the other files.
    let initial = db.input(initial_file_path.into())?;
    loop {
        // Compile the code starting at the provided input, this will read other
        // needed files using the on-demand mechanism.
        let sum = compile(&db, initial);
        let diagnostics = compile::accumulated::<Diagnostic>(&db, initial);
        if diagnostics.is_empty() {
            println!("Sum is: {}", sum);
        } else {
            for diagnostic in diagnostics {
                println!("{}", diagnostic.0);
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
            let file = match db.files.get(&path) {
                Some(file) => *file,
                None => continue,
            };
            // `path` has changed, so read it and update the contents to match.
            // This creates a new revision and causes the incremental algorithm
            // to kick in, just like any other update to a salsa input.
            let contents = std::fs::read_to_string(path)
                .wrap_err_with(|| format!("Failed to read file {}", event.path.display()))?;
            file.set_contents(&mut db).to(contents);
        }
    }
}
// ANCHOR_END: main

// ANCHOR: db
#[salsa::input]
struct File {
    path: PathBuf,
    #[return_ref]
    contents: String,
}

#[salsa::db]
trait Db: salsa::Database {
    fn input(&self, path: PathBuf) -> Result<File>;
}

#[salsa::db]
struct LazyInputDatabase {
    storage: Storage<Self>,
    logs: Mutex<Vec<String>>,
    files: DashMap<PathBuf, File>,
    file_watcher: Mutex<Debouncer<RecommendedWatcher>>,
}

impl LazyInputDatabase {
    fn new(tx: Sender<DebounceEventResult>) -> Self {
        Self {
            storage: Default::default(),
            logs: Default::default(),
            files: DashMap::new(),
            file_watcher: Mutex::new(new_debouncer(Duration::from_secs(1), tx).unwrap()),
        }
    }
}

#[salsa::db]
impl salsa::Database for LazyInputDatabase {
    fn salsa_event(&self, event: &dyn Fn() -> salsa::Event) {
        // don't log boring events
        let event = event();
        if let salsa::EventKind::WillExecute { .. } = event.kind {
            self.logs.lock().unwrap().push(format!("{:?}", event));
        }
    }
}

#[salsa::db]
impl Db for LazyInputDatabase {
    fn input(&self, path: PathBuf) -> Result<File> {
        let path = path
            .canonicalize()
            .wrap_err_with(|| format!("Failed to read {}", path.display()))?;
        Ok(match self.files.entry(path.clone()) {
            // If the file already exists in our cache then just return it.
            Entry::Occupied(entry) => *entry.get(),
            // If we haven't read this file yet set up the watch, read the
            // contents, store it in the cache, and return it.
            Entry::Vacant(entry) => {
                // Set up the watch before reading the contents to try to avoid
                // race conditions.
                let watcher = &mut *self.file_watcher.lock().unwrap();
                watcher
                    .watcher()
                    .watch(&path, RecursiveMode::NonRecursive)
                    .unwrap();
                let contents = std::fs::read_to_string(&path)
                    .wrap_err_with(|| format!("Failed to read {}", path.display()))?;
                *entry.insert(File::new(self, path, contents))
            }
        })
    }
}
// ANCHOR_END: db

#[salsa::accumulator]
struct Diagnostic(String);

impl Diagnostic {
    fn push_error(db: &dyn Db, file: File, error: Report) {
        Diagnostic(format!(
            "Error in file {}: {:?}\n",
            file.path(db)
                .file_name()
                .unwrap_or_else(|| "<unknown>".as_ref())
                .to_string_lossy(),
            error,
        ))
        .accumulate(db);
    }
}

#[salsa::tracked]
struct ParsedFile<'db> {
    value: u32,
    #[return_ref]
    links: Vec<ParsedFile<'db>>,
}

#[salsa::tracked]
fn compile(db: &dyn Db, input: File) -> u32 {
    let parsed = parse(db, input);
    sum(db, parsed)
}

#[salsa::tracked]
fn parse(db: &dyn Db, input: File) -> ParsedFile<'_> {
    let mut lines = input.contents(db).lines();
    let value = match lines.next().map(|line| (line.parse::<u32>(), line)) {
        Some((Ok(num), _)) => num,
        Some((Err(e), line)) => {
            Diagnostic::push_error(
                db,
                input,
                Report::new(e).wrap_err(format!(
                    "First line ({}) could not be parsed as an integer",
                    line
                )),
            );
            0
        }
        None => {
            Diagnostic::push_error(db, input, eyre!("File must contain an integer"));
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
                        input,
                        Report::new(err).wrap_err(format!("Failed to parse path: {}", path)),
                    );
                    return None;
                }
            };
            let link_path = input.path(db).parent().unwrap().join(relative_path);
            match db.input(link_path) {
                Ok(file) => Some(parse(db, file)),
                Err(err) => {
                    Diagnostic::push_error(db, input, err);
                    None
                }
            }
        })
        .collect();
    ParsedFile::new(db, value, links)
}

#[salsa::tracked]
fn sum<'db>(db: &'db dyn Db, input: ParsedFile<'db>) -> u32 {
    input.value(db)
        + input
            .links(db)
            .iter()
            .map(|&file| sum(db, file))
            .sum::<u32>()
}
