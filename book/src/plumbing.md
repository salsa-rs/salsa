# Plumbing

{{#include caveat.md}}

This chapter documents the code that salsa generates and its "inner workings".
We refer to this as the "plumbing".

## Overview

The plumbing section is broken up into chapters:

* The [jars and ingredients](./plumbing/jars_and_ingredients.md) covers how each salsa item (like a tracked function)
  specifies what data it needs and runtime, and how links between items work.
