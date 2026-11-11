//! The marks of the sessions and runs a browser's tabs have open
//! ([`crate::persist::Store::mark_open`]): each a key of `localStorage`,
//! held by a Web Lock of the same name for as long as its tab runs it,
//! through a page script of their own (`src/persist/marks.js`).

/// What a tab does with its own mark, and with those of tabs gone: the body
/// of a function of `prefix` and `options`, run on the page.
pub(super) const MARKS_JS: &str = include_str!("marks.js");

#[cfg(test)]
mod page_tests;
