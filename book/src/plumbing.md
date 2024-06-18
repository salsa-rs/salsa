# Plumbing

This chapter documents the code that salsa generates and its "inner workings".
We refer to this as the "plumbing".

## Overview

The plumbing section is broken up into chapters:

- The [jars and ingredients](./plumbing/jars_and_ingredients.md) covers how each salsa item (like a tracked function) specifies what data it needs and runtime, and how links between items work.
- The [database and runtime](./plumbing/database_and_runtime.md) covers the data structures that are used at runtime to coordinate workers, trigger cancellation, track which functions are active and what dependencies they have accrued, and so forth.
- The [query operations](./plumbing/query_ops.md) chapter describes how the major operations on function ingredients work. This text was written for an older version of salsa but the logic is the same:
  - The [maybe changed after](./plumbing/maybe_changed_after.md) operation determines when a memoized value for a tracked function is out of date.
  - The [fetch](./plumbing/fetch.md) operation computes the most recent value.
  - The [derived queries flowchart](./plumbing/derived_flowchart.md) depicts the logic in flowchart form.
  - The [cycle handling](./plumbing/cycles.md) handling chapter describes what happens when cycles occur.
- The [terminology](./plumbing/terminology.md) section describes various words that appear throughout.
