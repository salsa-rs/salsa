use std::{path::PathBuf, sync::Mutex};

use crossbeam_channel::{unbounded, Sender};
use dashmap::DashMap;
use notify::{
    event::ModifyKind, recommended_watcher, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use salsa::DebugWithDb;

fn main() {
    let (tx, rx) = unbounded();
    let mut db = Database::new(tx);
    let initial = db.input("./lazy-input/inputs/start".parse().unwrap());
    loop {
        let parsed = parse(&db, initial);
        let sum = sum(&db, parsed);

        println!("{}", sum);
        for log in db.logs.lock().unwrap().drain(..) {
            println!("{}", log);
        }

        loop {
            let event = rx.recv().unwrap().unwrap();
            let paths: Vec<_> = if event.need_rescan() {
                db.files.iter().map(|entry| entry.key().clone()).collect()
            } else if matches!(
                event.kind,
                EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
            ) {
                continue;
            } else {
                event.paths
            };
            if paths.is_empty() {
                continue;
            }
            for path in paths {
                let path = path.canonicalize().unwrap();
                let file = match db.files.get(&path) {
                    Some(file) => *file,
                    None => continue,
                };
                file.set_contents(&mut db)
                    .to(std::fs::read_to_string(path).unwrap());
            }
            break;
        }
    }
}

#[salsa::jar(db = Db)]
struct Jar(File, ParsedFile, parse, sum);

trait Db: salsa::DbWithJar<Jar> {
    fn input(&self, path: PathBuf) -> File;
}

#[salsa::db(Jar)]
struct Database {
    storage: salsa::Storage<Self>,
    logs: Mutex<Vec<String>>,
    files: DashMap<PathBuf, File>,
    file_watcher: Mutex<RecommendedWatcher>,
}

impl Database {
    fn new(tx: Sender<notify::Result<notify::Event>>) -> Self {
        let storage = Default::default();
        Self {
            storage,
            logs: Default::default(),
            files: DashMap::new(),
            file_watcher: Mutex::new(recommended_watcher(tx).unwrap()),
        }
    }
}

impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        // don't log boring events
        if let salsa::EventKind::WillExecute { .. } = event.kind {
            self.logs
                .lock()
                .unwrap()
                .push(format!("Event: {:?}", event.debug(self)));
        }
    }
}

impl Db for Database {
    fn input(&self, path: PathBuf) -> File {
        let path = path.canonicalize().unwrap();
        *self.files.entry(path.clone()).or_insert_with(|| {
            let watcher = &mut *self.file_watcher.lock().unwrap();
            watcher.watch(&path, RecursiveMode::NonRecursive).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            File::new(self, path, contents)
        })
    }
}

#[salsa::input]
struct File {
    path: PathBuf,
    #[return_ref]
    contents: String,
}

#[salsa::tracked]
fn parse(db: &dyn Db, input: File) -> ParsedFile {
    let mut lines = input.contents(db).lines();
    let value = lines.next().unwrap().parse().unwrap();
    let links = lines
        .map(|path| {
            let link_path = input
                .path(db)
                .parent()
                .unwrap()
                .join(path.parse::<PathBuf>().unwrap());
            let file = db.input(link_path);
            parse(db, file)
        })
        .collect();
    ParsedFile::new(db, value, links)
}

#[salsa::tracked]
struct ParsedFile {
    value: u32,
    #[return_ref]
    links: Vec<ParsedFile>,
}

#[salsa::tracked]
fn sum(db: &dyn Db, input: ParsedFile) -> u32 {
    input.value(db)
        + input
            .links(db)
            .iter()
            .map(|&file| sum(db, file))
            .sum::<u32>()
}
