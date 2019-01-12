use crate::constants;
use crate::counter::Counter;
use crate::log::Log;
use crate::memoized_dep_inputs;
use crate::memoized_inputs;
use crate::memoized_volatile;

pub(crate) trait TestContext: salsa::Database {
    fn clock(&self) -> &Counter;
    fn log(&self) -> &Log;
}

#[derive(Default)]
pub(crate) struct TestContextImpl {
    runtime: salsa::Runtime<TestContextImpl>,
    clock: Counter,
    log: Log,
}

impl TestContextImpl {
    pub(crate) fn assert_log(&self, expected_log: &[&str]) {
        let expected_text = &format!("{:#?}", expected_log);
        let actual_text = &format!("{:#?}", self.log().take());

        if expected_text == actual_text {
            return;
        }

        for diff in diff::lines(expected_text, actual_text) {
            match diff {
                diff::Result::Left(l) => println!("-{}", l),
                diff::Result::Both(l, _) => println!(" {}", l),
                diff::Result::Right(r) => println!("+{}", r),
            }
        }

        panic!("incorrect log results");
    }
}

salsa::database_storage! {
    pub(crate) struct TestContextImplStorage for TestContextImpl {
        impl constants::ConstantsDatabase {
            fn constants_input() for constants::ConstantsInputQuery;
            fn constants_derived() for constants::ConstantsAddQuery;
        }

        impl memoized_dep_inputs::MemoizedDepInputsContext {
            fn dep_memoized2() for memoized_dep_inputs::DepMemoized2Query;
            fn dep_memoized1() for memoized_dep_inputs::DepMemoized1Query;
            fn dep_derived1() for memoized_dep_inputs::DepDerived1Query;
            fn dep_input1() for memoized_dep_inputs::DepInput1Query;
            fn dep_input2() for memoized_dep_inputs::DepInput2Query;
        }

        impl memoized_inputs::MemoizedInputsContext {
            fn max() for memoized_inputs::MaxQuery;
            fn input1() for memoized_inputs::Input1Query;
            fn input2() for memoized_inputs::Input2Query;
        }

        impl memoized_volatile::MemoizedVolatileContext {
            fn memoized2() for memoized_volatile::Memoized2Query;
            fn memoized1() for memoized_volatile::Memoized1Query;
            fn volatile() for memoized_volatile::VolatileQuery;
        }
    }
}

impl TestContext for TestContextImpl {
    fn clock(&self) -> &Counter {
        &self.clock
    }

    fn log(&self) -> &Log {
        &self.log
    }
}

impl salsa::Database for TestContextImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<TestContextImpl> {
        &self.runtime
    }
}
