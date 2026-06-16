//! A compiler/IDE-shaped benchmark with many files, tracked declarations, and diagnostics.
//!
//! Every non-root file imports the root file. A private edit changes declaration bodies while
//! preserving their signatures; a public edit changes signatures and invalidates every importer.

use std::hint::black_box;

use codspeed_criterion_compat::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main,
};
use salsa::{Accumulator, Setter};

const FILES: usize = 1_024;
const DECLARATIONS_PER_FILE: usize = 16;
const DIAGNOSTIC_INTERVAL: usize = 128;
const DIAGNOSTICS: usize = FILES / DIAGNOSTIC_INTERVAL;

#[salsa::input]
struct SourceFile {
    module: usize,

    #[returns(ref)]
    text: String,

    #[returns(ref)]
    dependencies: Vec<SourceFile>,
}

#[salsa::input]
struct Workspace {
    #[returns(ref)]
    files: Vec<SourceFile>,
}

#[salsa::tracked]
struct Declaration<'db> {
    /// Stable identity within a file.
    name: usize,

    #[tracked]
    signature: usize,

    #[tracked]
    body: usize,
}

#[salsa::accumulator]
struct Diagnostic(#[allow(dead_code)] usize);

#[salsa::tracked(returns(ref))]
#[inline(never)]
fn parse_file(db: &dyn salsa::Database, file: SourceFile) -> Vec<Declaration<'_>> {
    let (api_seed, body_seed, declarations) = parse_source(file.text(db));
    (0..declarations)
        .map(|name| Declaration::new(db, name, mix(api_seed, name), mix(body_seed, name)))
        .collect()
}

#[salsa::tracked]
#[inline(never)]
fn public_api(db: &dyn salsa::Database, file: SourceFile) -> usize {
    parse_file(db, file)
        .iter()
        .fold(0usize, |checksum, &declaration| {
            checksum.wrapping_add(declaration.signature(db))
        })
}

#[salsa::tracked]
#[inline(never)]
fn check_file(db: &dyn salsa::Database, file: SourceFile) -> usize {
    let mut checksum = public_api(db, file);
    for &declaration in parse_file(db, file) {
        checksum = checksum.wrapping_add(declaration.body(db));
    }
    for &dependency in file.dependencies(db) {
        checksum = checksum.wrapping_add(public_api(db, dependency));
    }

    let module = file.module(db);
    if module % DIAGNOSTIC_INTERVAL == 0 {
        Diagnostic(module).accumulate(db);
    }

    checksum
}

#[salsa::tracked]
#[inline(never)]
fn check_workspace(db: &dyn salsa::Database, workspace: Workspace) -> usize {
    workspace.files(db).iter().fold(0usize, |checksum, &file| {
        checksum.wrapping_add(check_file(db, file))
    })
}

fn parse_source(source: &str) -> (usize, usize, usize) {
    let mut fields = source.split(':');
    let api = fields.next().unwrap().parse().unwrap();
    let body = fields.next().unwrap().parse().unwrap();
    let declarations = fields.next().unwrap().parse().unwrap();
    assert!(fields.next().is_none());
    (api, body, declarations)
}

fn source_text(api: usize, body: usize) -> String {
    format!("{api}:{body}:{DECLARATIONS_PER_FILE}")
}

fn mix(seed: usize, index: usize) -> usize {
    seed.wrapping_mul(1_664_525)
        .wrapping_add(index)
        .wrapping_add(1_013_904_223)
}

struct WorkspaceFixture {
    db: salsa::DatabaseImpl,
    workspace: Workspace,
    edited_file: SourceFile,
    initial_checksum: usize,
}

impl WorkspaceFixture {
    fn new() -> Self {
        let db = salsa::DatabaseImpl::new();
        let root = SourceFile::new(&db, 0, source_text(1, 100), Vec::new());

        let mut files = Vec::with_capacity(FILES);
        files.push(root);
        for module in 1..FILES {
            files.push(SourceFile::new(
                &db,
                module,
                source_text(module + 1, module + 100),
                vec![root],
            ));
        }

        let workspace = Workspace::new(&db, files);
        let initial_checksum = check_workspace(&db, workspace);
        assert_ne!(initial_checksum, 0);
        let diagnostics = check_workspace::accumulated::<Diagnostic>(&db, workspace);
        assert_eq!(diagnostics.len(), DIAGNOSTICS);

        Self {
            db,
            workspace,
            edited_file: root,
            initial_checksum,
        }
    }

    fn run(&mut self, api: usize, body: usize) {
        self.edited_file
            .set_text(black_box(&mut self.db))
            .to(black_box(source_text(api, body)));

        let checksum = check_workspace(black_box(&self.db), black_box(self.workspace));
        assert_ne!(black_box(checksum), self.initial_checksum);

        let diagnostics = check_workspace::accumulated::<Diagnostic>(
            black_box(&self.db),
            black_box(self.workspace),
        );
        assert_eq!(black_box(diagnostics.len()), DIAGNOSTICS);
    }
}

fn workspace_edits(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("Workspace");

    for (name, api, body) in [("private_edit", 1, 101), ("public_api_edit", 2, 100)] {
        group.bench_function(BenchmarkId::new(name, FILES), move |b| {
            b.iter_batched_ref(
                WorkspaceFixture::new,
                |fixture| fixture.run(api, body),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, workspace_edits);
criterion_main!(benches);
