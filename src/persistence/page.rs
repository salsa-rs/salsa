//! Reuse complete, reachable ranges of encoded rows from the previous snapshot.

use super::{IdentityRows, encode_references, invalid, visit_references, with_references};
use crate::ingredient::Ingredient;
use crate::zalsa::{IngredientIndex, Zalsa};
use crate::{DatabaseKeyIndex, Id};
use bytes::Bytes;
use rustc_hash::FxHashMap;
use std::io;

pub(super) type SavedPages = FxHashMap<(u32, u64), Page>;

#[derive(Clone)]
pub(super) struct Page {
    pub data: Bytes,
    pub rows: Bytes,
    pub references: Bytes,
}

pub(super) struct Block {
    pub index: u32,
    pub first: u64,
    pub page: Page,
}

/// Partition by slot range so that inserting one key does not move all subsequent rows.
pub(super) fn slot_range(key: &DatabaseKeyIndex) -> u32 {
    key.key_index().index() / 128
}

pub(super) fn encode(
    zalsa: &Zalsa,
    index: IngredientIndex,
    ingredient: &dyn Ingredient,
    keys: &[DatabaseKeyIndex],
    identities: &IdentityRows,
    previous: Option<&SavedPages>,
) -> io::Result<(Block, Vec<Id>)> {
    let index = index.as_u32();
    let first = keys[0].key_index().as_bits();
    let old = previous.and_then(|pages| pages.get(&(index, first)));
    let mut rows = Vec::new();
    let mut unchanged = old.is_some();
    if ingredient.supports_cache_pages() && keys.len() >= 4 {
        let function = ingredient.as_function().is_some();
        let mut previous_id = 0;
        let mut previous_stamp = 0;
        for &key in keys {
            let id = key.key_index().as_bits();
            push(id ^ previous_id, &mut rows);
            previous_id = id;
            if !function || unchanged {
                // SAFETY: write_cache holds exclusive access throughout encoding.
                let stamp = unsafe { ingredient.cache_page_stamp(zalsa, key) };
                if function {
                    unchanged &= stamp.is_some();
                } else if let Some(stamp) = stamp {
                    push(stamp ^ previous_stamp, &mut rows);
                    previous_stamp = stamp;
                } else {
                    rows.clear();
                    unchanged = false;
                    break;
                }
            }
        }
    }
    if let Some(old) = old
        && unchanged
        && !rows.is_empty()
        && old.rows == rows
    {
        let mut references = Vec::new();
        visit_references(&old.references, |id| references.push(id))?;
        return Ok((
            Block {
                index,
                first,
                page: old.clone(),
            },
            references,
        ));
    }
    let (data, mut references) = with_references(|| {
        // SAFETY: The snapshot writer excludes mutations while the workers are active.
        unsafe { ingredient.encode_cache(zalsa, keys, identities) }
    });
    let page = Page {
        data: data?.into(),
        references: if rows.is_empty() {
            Bytes::new()
        } else {
            encode_references(&mut references)?
        },
        rows: rows.into(),
    };
    Ok((Block { index, first, page }, references))
}

fn push(value: impl bincode::Encode, bytes: &mut Vec<u8>) {
    bincode::encode_into_std_write(value, bytes, bincode::config::standard())
        .expect("writing a row identifier to a Vec");
}

pub(super) type Directory<'a> = Vec<(usize, u64, &'a [u8], &'a [u8])>;

pub(super) fn directory(blocks: &[Block]) -> Directory<'_> {
    blocks
        .iter()
        .enumerate()
        .filter_map(|(position, block)| {
            (!block.page.rows.is_empty()).then_some((
                position,
                block.first,
                block.page.rows.as_ref(),
                block.page.references.as_ref(),
            ))
        })
        .collect()
}

pub(super) fn restore(
    bytes: &Bytes,
    blocks: &[(u32, &[u8])],
    directory: Directory<'_>,
) -> io::Result<SavedPages> {
    let mut pages = SavedPages::default();
    for (position, first, rows, references) in directory {
        let &(index, data) = blocks
            .get(position)
            .ok_or_else(|| invalid("invalid cache page"))?;
        let page = Page {
            data: bytes.slice_ref(data),
            rows: bytes.slice_ref(rows),
            references: bytes.slice_ref(references),
        };
        if rows.is_empty() || pages.insert((index, first), page).is_some() {
            return Err(invalid("invalid cache page directory"));
        }
    }
    Ok(pages)
}
