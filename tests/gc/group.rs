use crate::log::HasLog;

salsa::query_group! {
    pub(crate) trait GcDatabase: salsa::Database + HasLog {
        fn min() -> usize {
            type Min;
            storage input;
        }

        fn max() -> usize {
            type Max;
            storage input;
        }

        fn use_triangular(key: usize) -> bool {
            type UseTriangular;
            storage (input default);
        }

        fn fibonacci(key: usize) -> usize {
            type Fibonacci;
        }

        fn triangular(key: usize) -> usize {
            type Triangular;
        }

        fn compute(key: usize) -> usize {
            type Compute;
        }

        fn compute_all() -> Vec<usize> {
            type ComputeAll;
        }
    }
}

fn fibonacci(db: &impl GcDatabase, key: usize) -> usize {
    db.log().add(format!("fibonacci({:?})", key));
    if key == 0 {
        0
    } else if key == 1 {
        1
    } else {
        db.fibonacci(key - 1) + db.fibonacci(key - 2)
    }
}

fn triangular(db: &impl GcDatabase, key: usize) -> usize {
    db.log().add(format!("triangular({:?})", key));
    if key == 0 {
        0
    } else {
        db.triangular(key - 1) + key
    }
}

fn compute(db: &impl GcDatabase, key: usize) -> usize {
    db.log().add(format!("compute({:?})", key));
    if db.use_triangular(key) {
        db.triangular(key)
    } else {
        db.fibonacci(key)
    }
}

fn compute_all(db: &impl GcDatabase) -> Vec<usize> {
    db.log().add("compute_all()");
    (db.min()..db.max()).map(|v| db.compute(v)).collect()
}
