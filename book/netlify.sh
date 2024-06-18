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

mdbook build
mkdir versions
mv book/html/* versions
