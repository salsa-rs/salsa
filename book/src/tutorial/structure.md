# Basic structure

Before we do anything with Salsa, let's talk about the basic structure of the calc compiler.
Part of Salsa's design is that you are able to write programs that feel 'pretty close' to what a natural Rust program looks like.

## Example program

This is our example calc program:

```
x = 5
y = 10
z = x + y * 3
print z
```

## Parser

The calc compiler takes as input a program, represented by a string:

```rust
struct ProgramSource {
    text: String
}
```

The first thing it does it to parse that string into a series of statements that look something like the following pseudo-Rust:[^lexer]

```rust
enum Statement {
    /// Defines `fn <name>(<args>) = <body>`
    Function(Function),
    /// Defines `print <expr>`
    Print(Expression),
}

/// Defines `fn <name>(<args>) = <body>`
struct Function {
    name: FunctionId,
    args: Vec<VariableId>,
    body: Expression
}
```

where an expression is something like this (pseudo-Rust, because the `Expression` enum is recursive):

```rust
enum Expression {
    Op(Expression, Op, Expression),
    Number(f64),
    Variable(VariableId),
    Call(FunctionId, Vec<Expression>),
}

enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
}
```

Finally, for function/variable names, the `FunctionId` and `VariableId` types will be interned strings:

```rust
type FunctionId = /* interned string */;
type VariableId = /* interned string */;
```

[^lexer]: Because calc is so simple, we don't have to bother separating out the lexer from the parser.

## Type Checker

The "type checker" has the job of ensuring that the user only references variables and functions that have been defined.

## Interpreter

The interpreter will execute the program and print the result.
Mostly for demonstration purposes, we have set it up so that
it will avoid re-executing functions if their body has not changed.