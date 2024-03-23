use std::fmt::{self, Display, Formatter};

pub trait DisplayWithDb<'db, DB: ?Sized + 'db> {
    fn fmt_with(&self, db: &DB, f: &mut Formatter<'_>) -> fmt::Result;

    fn display_with(&self, db: &DB) -> impl Display {
        FormatterFn(|f| self.fmt_with(db, f))
    }

    fn to_string_with(&self, db: &DB) -> String {
        self.display_with(db).to_string()
    }
}

// TODO: replace with `[std::fmt::FormatterFn]` when it becomes stable
struct FormatterFn<F>(pub F)
where
    F: Fn(&mut fmt::Formatter<'_>) -> fmt::Result;

impl<F> Display for FormatterFn<F>
where
    F: Fn(&mut fmt::Formatter<'_>) -> fmt::Result,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (self.0)(f)
    }
}
