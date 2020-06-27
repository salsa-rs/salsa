# RFCs

The Salsa RFC process is used to describe the motivations for major changes made
to Salsa. RFCs are recorded here in the Salsa book as a historical record of the
considerations that were raised at the time. Note that the contents of RFCs,
once merged, is typically not updated to match further changes. Instead, the
rest of the book is updated to include the RFC text and then kept up to
date as more PRs land and so forth.

## Creating an RFC

If you'd like to propose a major new Salsa feature, simply clone the repository
and create a new chapter under the list of RFCs based on the [RFC template].
Then open a PR with a subject line that starts with "RFC:".

[RFC template]: ./rfcs/template.md

## RFC vs Implementation

The RFC can be in its own PR, or it can also includ work on the implementation
together, whatever works best for you.

## Does my change need an RFC?

Not all PRs require RFCs. RFCs are only needed for larger features or major
changes to how Salsa works. And they don't have to be super complicated, but
they should capture the most important reasons you would like to make the
change. When in doubt, it's ok to just open a PR, and we can always request an
RFC if we want one.