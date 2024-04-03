#!/bin/bash
#
# Script meant to be run from netlify

set -x

MDBOOK_VERSION='0.4.12'
MDBOOK_LINKCHECK_VERSION='0.7.4'
MDBOOK_MERMAID_VERSION='0.8.3'

curl -L https://github.com/rust-lang/mdBook/releases/download/v$MDBOOK_VERSION/mdbook-v$MDBOOK_VERSION-x86_64-unknown-linux-gnu.tar.gz | tar xz -C ~/.cargo/bin
curl -L https://github.com/badboy/mdbook-mermaid/releases/download/v$MDBOOK_MERMAID_VERSION/mdbook-mermaid-v$MDBOOK_MERMAID_VERSION-x86_64-unknown-linux-gnu.tar.gz | tar xz -C ~/.cargo/bin
curl -L https://github.com/Michael-F-Bryan/mdbook-linkcheck/releases/download/v$MDBOOK_LINKCHECK_VERSION/mdbook-linkcheck.v$MDBOOK_LINKCHECK_VERSION.x86_64-unknown-linux-gnu.zip -O
unzip mdbook-linkcheck.v$MDBOOK_LINKCHECK_VERSION.x86_64-unknown-linux-gnu.zip -d ~/.cargo/bin
chmod +x ~/.cargo/bin/mdbook-linkcheck

# ======================================================================
# The following script automates the deployment of both the latest and a
# specified older version of the 'salsa' documentation using mdbook

# Store the current branch or commit
original_branch=$(git rev-parse --abbrev-ref HEAD)
if [ "$original_branch" == "HEAD" ]; then
  original_branch=$(git rev-parse HEAD)
fi

mkdir -p versions  # Create a root directory for all versions

# Declare an associative array to map commits to custom version directory names
declare -A commit_to_version=( ["$original_branch"]="salsa2022" ["754eea8b5f8a31b1100ba313d59e41260b494225"]="salsa" )

# Loop over the keys (commit hashes or branch names) in the associative array
for commit in "${!commit_to_version[@]}"; do
  git checkout $commit
  mdbook build
  version_dir="versions/${commit_to_version[$commit]}"
  mkdir -p $version_dir
  mv book/html/* $version_dir
  rm -rf book
done

# Return to the original branch or commit
git checkout $original_branch

# Copy _redirects to the root directory
cp _redirects versions
